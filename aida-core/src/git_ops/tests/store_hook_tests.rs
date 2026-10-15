use super::*;

// Store git ignores project hooks, even when they resolve in the store.
// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_relative_hooks_path_never_runs_project_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    git(&main, &["config", "core.hooksPath", ".husky/_"]).unwrap();
    let main_marker = dir.path().join("MAIN-HOOK-RAN");
    let planted_marker = dir.path().join("PLANTED-HOOK-RAN");
    bug_1929_marker_hook(&main.join(".husky/_/commit-msg"), &main_marker, "main");
    bug_1929_marker_hook(
        &store.join(".husky/_/commit-msg"),
        &planted_marker,
        "planted",
    );
    let hooks = resolve_store_hooks_dir(&store).unwrap().unwrap();
    assert!(hooks.starts_with(main.join(".git")));
    assert_eq!(hooks_dir(&store).unwrap(), hooks);
    crate::db::with_store_write_lock(&store, || {
        std::fs::write(store.join("notes.txt"), "one\n")?;
        add(&store, &["notes.txt"])?;
        commit_paths(&store, "chore: store write", &["notes.txt"])
    })
    .unwrap();
    assert!(!planted_marker.exists());
    assert!(
        !main_marker.exists(),
        "project hooks must never run on store commits"
    );
    assert!(host_git_hardening(&store).contains(&format!("core.hooksPath={}", hooks.display())));
}

// A hostile existing profile refuses before staging/committing, never repairs
// itself, and even a direct builder invocation fails. Own repos use only .git.
// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_unverified_host_hooks_refuse_store_writes() {
    use std::os::unix::fs::PermissionsExt;
    for corruption in [
        "bytes",
        "symlink",
        "extra",
        "directory-mode",
        "file-mode",
        "hardlink",
        "directory-symlink",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (_main, store) = bug_1929_main_with_store(dir.path());
        let hooks = resolve_store_hooks_dir(&store).unwrap().unwrap();
        let hook = hooks.join("commit-msg");
        match corruption {
            "bytes" => {
                std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
            }
            "symlink" => {
                let copy = dir.path().join("copy");
                std::fs::copy(&hook, &copy).unwrap();
                std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::remove_file(&hook).unwrap();
                std::os::unix::fs::symlink(copy, &hook).unwrap();
            }
            "extra" => {
                std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::write(hooks.join("pre-rebase"), "#!/bin/sh\nexit 0\n").unwrap();
            }
            "directory-mode" => {
                std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o777)).unwrap()
            }
            "file-mode" => {
                std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o777)).unwrap()
            }
            "hardlink" => std::fs::hard_link(&hook, dir.path().join("hardlink")).unwrap(),
            "directory-symlink" => {
                let moved = dir.path().join("moved-hooks");
                std::fs::rename(&hooks, &moved).unwrap();
                std::os::unix::fs::symlink(moved, &hooks).unwrap();
            }
            _ => unreachable!(),
        }
        std::fs::write(store.join("notes.txt"), "pending\n").unwrap();
        assert!(resolve_store_hooks_dir(&store).is_err(), "{corruption}");
        assert!(
            ensure_store_hooks_supported(&store).is_err(),
            "{corruption}"
        );
        assert!(add(&store, &["notes.txt"]).is_err(), "{corruption}");
        assert!(
            commit_paths(&store, "chore: x", &["notes.txt"]).is_err(),
            "{corruption}"
        );
        let out = host_git_command(&store)
            .args(["commit", "-am", "chore: x"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{corruption}: {out:?}");
        // The refusal names the failed check, never a false "detached".
        // trace:BUG-1929 | ai:claude
        let shim = String::from_utf8_lossy(&out.stderr);
        assert!(
            shim.contains("cannot establish verified host-owned AIDA hooks")
                && shim.contains(&*hooks.to_string_lossy()),
            "{corruption}: {shim}"
        );
        let safe = format!("{:#}", ensure_store_write_safe(&store).unwrap_err());
        assert!(!safe.contains("detached"), "{corruption}: {safe}");
        assert!(
            safe.contains("cannot establish verified"),
            "{corruption}: {safe}"
        );
        assert_eq!(
            store_worktree_issue(&store),
            Some("host-owned AIDA store hooks could not be verified"),
            "{corruption}"
        );
        let staged = Command::new("git")
            .current_dir(&store)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(staged.stdout.is_empty(), "{corruption}: nothing staged");
        // Permit TempDir cleanup after intentionally read-only installation.
        if corruption != "directory-symlink" {
            std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join(".aida-store");
    init(&own).unwrap();
    assert!(resolve_store_hooks_dir(&own)
        .unwrap()
        .unwrap()
        .starts_with(own.join(".git")));
    // Moving git's authority into writable store content is not exempt.
    let moved_git = own.join("writable-git");
    std::fs::rename(own.join(".git"), &moved_git).unwrap();
    std::fs::write(
        own.join(".git"),
        format!("gitdir: {}\n", moved_git.display()),
    )
    .unwrap();
    assert!(resolve_store_hooks_dir(&own)
        .unwrap_err()
        .to_string()
        .contains("inside the store"));
}

// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_host_git_command_marks_store_commits() {
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    let guard = |cmd: &Command| {
        cmd.get_envs()
            .find(|(k, _)| *k == "AIDA_STORE_WRITE_GUARD")
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().to_string()))
    };
    assert_eq!(
        guard(&host_git_command(&store)).as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(guard(&host_git_command(&main)), None);
    std::fs::write(store.join("notes.txt"), "one\n").unwrap();
    assert!(host_git_command(&store)
        .args(["add", "notes.txt"])
        .status()
        .unwrap()
        .success());
    assert!(host_git_command(&store)
        .args(["commit", "-q", "-m", "chore: direct"])
        .status()
        .unwrap()
        .success());
}

// Real commit_paths with the pin removed on this test thread only. The
// security oracle is identical for production and mutant; the marker shows
// the changed command ran, rather than a no-op source mutation.
// trace:BUG-1929 | ai:codex
#[cfg(unix)]
fn bug_1929_manager_fixture(manager: &str, mutant: bool) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    let project_hooks = main.join("project-hooks");
    std::fs::create_dir_all(&project_hooks).unwrap();
    git(
        &main,
        &["config", "core.hooksPath", project_hooks.to_str().unwrap()],
    )
    .unwrap();
    let marker = dir.path().join("HOST-EXECUTED-PLANT");
    let executable = |path: &Path, body: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    match manager {
        "husky" => {
            // Husky v9: h resolves .husky/<hook> from cwd and prepends
            // node_modules/.bin. The project-owned h is not itself planted.
            executable(&main.join(".husky/_/h"), "#!/bin/sh\nn=$(basename \"$0\")\ns=\".husky/$n\"\nexport PATH=\"node_modules/.bin:$PATH\"\nsh -e \"$s\" \"$@\"\n");
            executable(
                &project_hooks.join("pre-commit"),
                &format!("#!/bin/sh\n. '{}'\n", main.join(".husky/_/h").display()),
            );
            executable(&store.join(".husky/pre-commit"), "#!/bin/sh\nlint-staged\n");
            bug_1929_marker_hook(
                &store.join("node_modules/.bin/lint-staged"),
                &marker,
                "husky",
            );
            // Also plant the same h in the store, the relative hooksPath
            // attack. The dedicated relative-path regression covers it.
            executable(
                &store.join(".husky/_/h"),
                "#!/bin/sh\nexport PATH=\"node_modules/.bin:$PATH\"\nlint-staged\n",
            );
        }
        "pre-commit" => {
            // A small framework stand-in consumes the actual config shape
            // and executes the planted local hook's entry, from store cwd.
            executable(&project_hooks.join("pre-commit"), "#!/bin/sh\nentry=$(sed -n 's/^[[:space:]]*entry: //p' .pre-commit-config.yaml)\nsh -c \"$entry\"\n");
            std::fs::write(store.join(".pre-commit-config.yaml"), "repos:\n  - repo: local\n    hooks:\n      - id: planted\n        name: planted\n        language: system\n        entry: ./planted-tool\n").unwrap();
            bug_1929_marker_hook(&store.join("planted-tool"), &marker, "pre-commit");
        }
        "lefthook" => {
            executable(
                &project_hooks.join("pre-commit"),
                "#!/bin/sh\nrun=$(sed -n 's/^[[:space:]]*run: //p' lefthook.yml)\nsh -c \"$run\"\n",
            );
            std::fs::write(
                store.join("lefthook.yml"),
                "pre-commit:\n  commands:\n    planted:\n      run: ./planted-tool\n",
            )
            .unwrap();
            bug_1929_marker_hook(&store.join("planted-tool"), &marker, "lefthook");
        }
        _ => unreachable!(),
    }
    // Plant project-local binaries that the generic shipped pre-commit
    // would probe if its guarded-store early exit were lost.
    bug_1929_marker_hook(&store.join("target/debug/aida"), &marker, "local-aida");
    let hooks = resolve_store_hooks_dir(&store).unwrap().unwrap();
    for (name, key) in [
        ("pre-commit", "hooks/aida-pre-commit.sh"),
        ("commit-msg", "hooks/aida-commit-msg"),
    ] {
        assert_eq!(
            std::fs::read_to_string(hooks.join(name)).unwrap(),
            crate::templates::EMBEDDED_TEMPLATES[key]
        );
    }
    // An invalid message must be refused by the shipped commit-msg hook.
    // A commit of the same staged note with a valid message then succeeds.
    crate::db::with_store_write_lock(&store, || {
        std::fs::write(store.join("notes.txt"), "one\n")?;
        add(&store, &["notes.txt"])?;
        let out = host_git_command(&store)
            .env("AIDA_COMMIT_STRICT", "true")
            .args(["commit", "-m", "invalid message"])
            .output()?;
        anyhow::ensure!(
            !out.status.success(),
            "shipped commit-msg did not refuse invalid message"
        );
        anyhow::ensure!(
            String::from_utf8_lossy(&out.stderr).contains("conventional"),
            "wrong hook refusal: {out:?}"
        );
        // pre-commit writes the actual staged tree to the host sentinel;
        // post-commit cannot have consumed it on the refused commit.
        let sentinel = std::fs::read_to_string(main.join(".git/aida-precommit-sentinel"))?;
        let tree = git(&store, &["write-tree"])?;
        anyhow::ensure!(
            sentinel.trim() == tree.stdout,
            "shipped pre-commit did not run"
        );
        if mutant {
            let original = host_git_command(&store)
                .get_args()
                .map(|arg| arg.to_os_string())
                .collect::<Vec<_>>();
            DROP_STORE_HOOK_PIN.with(|flag| flag.set(true));
            let changed = host_git_command(&store)
                .get_args()
                .map(|arg| arg.to_os_string())
                .collect::<Vec<_>>();
            anyhow::ensure!(
                original != changed,
                "pin mutation changed no command arguments"
            );
            anyhow::ensure!(
                !changed
                    .iter()
                    .any(|arg| arg.to_string_lossy().starts_with("core.hooksPath=")),
                "pin mutation did not land"
            );
        }
        let committed = commit_paths(&store, "chore: guarded store commit", &["notes.txt"]);
        DROP_STORE_HOOK_PIN.with(|flag| flag.set(false));
        committed
    })
    .unwrap();
    let security_oracle = !marker.exists();
    if mutant {
        assert!(!security_oracle, "mutation was not exercised: {manager}");
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), manager);
        eprintln!("MUTATION KILLED: {manager}: dropping core.hooksPath fails the no-host-execution oracle; marker={}", marker.display());
    } else {
        assert!(
            security_oracle,
            "{manager}: sandbox-planted input executed on host"
        );
    }
}

// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_planted_husky_real_commit_paths_and_pin_mutation() {
    bug_1929_manager_fixture("husky", false);
    bug_1929_manager_fixture("husky", true);
}

// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_planted_pre_commit_real_commit_paths_and_pin_mutation() {
    bug_1929_manager_fixture("pre-commit", false);
    bug_1929_manager_fixture("pre-commit", true);
}

// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_planted_lefthook_real_commit_paths_and_pin_mutation() {
    bug_1929_manager_fixture("lefthook", false);
    bug_1929_manager_fixture("lefthook", true);
}

// trace:BUG-1929 | ai:codex
#[cfg(unix)]
#[test]
fn bug_1929_host_path_and_attribute_source() {
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    std::fs::create_dir_all(store.join("bin")).unwrap();
    let unsafe_path = format!(":.:bin:{}:/usr/bin::/bin", store.join("bin").display());
    let safe = crate::store_hooks::safe_path(&store, std::ffi::OsStr::new(&unsafe_path));
    assert_eq!(
        std::env::split_paths(&safe).collect::<Vec<_>>(),
        vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")]
    );
    let empty = crate::store_hooks::safe_path(&store, std::ffi::OsStr::new(":.:bin::"));
    assert_eq!(empty, std::ffi::OsStr::new("/usr/bin:/bin"));
    assert!(std::env::split_paths(&empty).all(|p| p.is_absolute()));
    let command = host_git_command(&store);
    let path = command
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .unwrap()
        .1
        .unwrap();
    assert!(std::env::split_paths(path).all(|p| p.is_absolute()));
    let attr = command
        .get_envs()
        .find(|(key, _)| *key == "GIT_ATTR_SOURCE");
    assert_eq!(
        attr.is_some(),
        crate::store_hooks::supports_attr_source(&store)
    );
    assert!(host_git_command(&main)
        .get_envs()
        .all(|(key, _)| key != "PATH" && key != "GIT_ATTR_SOURCE"));
    if attr.is_some() {
        std::fs::write(store.join(".gitattributes"), "*.txt custom-plant\n").unwrap();
        let out = host_git_command(&store)
            .args(["check-attr", "custom-plant", "--", "notes.txt"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("unspecified"),
            "{out:?}"
        );
    }
}

// Group-write is safe only on a user-owned directory whose group is the user's
// own primary group (the user-private-group default: 0775 user:user). World-write
// and group-write by any other group still refuse. trace:BUG-1929 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1929_only_owner_may_write_matrix() {
    use crate::store_hooks::only_owner_may_write as ok;
    let (me, my_group, other) = (1000, 1000, 2000);
    assert!(ok(me, my_group, 0o755, me, my_group));
    assert!(
        ok(me, my_group, 0o775, me, my_group),
        "user-private group 0775 is owner-only"
    );
    assert!(
        !ok(me, other, 0o775, me, my_group),
        "a shared group could swap hooks"
    );
    assert!(
        !ok(other, my_group, 0o775, me, my_group),
        "another owner's group-write"
    );
    assert!(!ok(me, my_group, 0o777, me, my_group), "world-writable");
    assert!(
        !ok(me, my_group, 0o757, me, my_group),
        "world-writable without group-write"
    );
    assert!(
        ok(me, other, 0o755, me, my_group),
        "no group-write: the group does not matter"
    );
}

// Reproduces the r8 blocker: on a host whose checkout and .git are 0775
// user:user, store writes must keep working; making .git world-writable must
// still refuse. trace:BUG-1929 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1929_user_private_group_0775_checkout_allows_store_writes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    let set = |p: &std::path::Path, mode: u32| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap()
    };
    for p in [dir.path(), main.as_path(), main.join(".git").as_path()] {
        set(p, 0o775);
    }
    let hooks = resolve_store_hooks_dir(&store).unwrap().unwrap();
    assert!(hooks.starts_with(main.join(".git")));
    crate::db::with_store_write_lock(&store, || {
        std::fs::write(store.join("notes.txt"), "0775\n")?;
        add(&store, &["notes.txt"])?;
        commit_paths(
            &store,
            "chore: store write on 0775 checkout",
            &["notes.txt"],
        )
    })
    .expect("a 0775 user:user checkout must not block store writes");
    set(&main.join(".git"), 0o777);
    assert!(
        resolve_store_hooks_dir(&store).is_err(),
        "a world-writable .git must still refuse"
    );
    set(&main.join(".git"), 0o775);
}

// uid_map parsing: only a well-formed non-identity map relaxes anything.
// Malformed or empty input counts as identity (no relaxation).
// trace:BUG-1929 | ai:claude
#[test]
fn bug_1929_uid_map_identity_parsing() {
    use crate::store_hooks::uid_map_is_identity as identity;
    assert!(identity("         0          0 4294967295\n"));
    assert!(identity("0 0 4294967295"));
    assert!(identity("\t0\t0\t4294967295  \n\n"));
    // Typical bwrap --unshare-user map: own uid only.
    assert!(!identity("      1000       1000          1\n"));
    // Rootless-container style multi-line map.
    assert!(!identity("0 1000 1\n1 100000 65536\n"));
    // Identity line plus another mapping is not the single identity line.
    assert!(!identity("0 0 4294967295\n1 1 1\n"));
    assert!(!identity("0 0 4294967294\n"));
    // Malformed: never treated as a namespace.
    assert!(identity(""));
    assert!(identity("garbage"));
    assert!(identity("1000 1000\n"));
    assert!(identity("1000 1000 1 7\n"));
    assert!(identity("1000 1000 x\n"));
    assert!(identity("1000 1000 1\nnot a map\n"));
    assert!(identity("-1 1000 1\n"));
}

// Ancestor owner/mode matrix: the overflow uid is an acceptable ancestor owner
// only inside a user namespace and only with the root mode rule.
// trace:BUG-1929 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1929_ancestor_overflow_uid_matrix() {
    use crate::store_hooks::check_ancestor as check;
    let (me, my_group, overflow, other) = (1000, 1000, 65534, 2000);
    let ns = Some(overflow);
    // Overflow-owned / and /home inside bwrap.
    assert!(check(overflow, overflow, 0o755, me, my_group, ns).is_ok());
    assert!(
        check(overflow, overflow, 0o1777, me, my_group, ns).is_ok(),
        "sticky world-writable like /tmp"
    );
    // Outside a namespace the overflow uid is just another user.
    let err = check(overflow, overflow, 0o755, me, my_group, None).unwrap_err();
    assert!(err.contains("untrusted owner uid 65534"), "{err}");
    // Unsafe modes refuse even inside a namespace.
    let err = check(overflow, overflow, 0o775, me, my_group, ns).unwrap_err();
    assert!(err.contains("group- or world-writable"), "{err}");
    assert!(check(overflow, overflow, 0o757, me, my_group, ns).is_err());
    assert!(check(overflow, overflow, 0o777, me, my_group, ns).is_err());
    // A different overflow value does not admit 65534.
    assert!(check(overflow, overflow, 0o755, me, my_group, Some(4242)).is_err());
    // Other owners always refuse, namespace or not.
    let err = check(other, other, 0o755, me, my_group, ns).unwrap_err();
    assert!(err.contains("untrusted owner uid 2000"), "{err}");
    // Root rule unchanged.
    assert!(check(0, 0, 0o755, me, my_group, None).is_ok());
    assert!(check(0, 0, 0o1777, me, my_group, None).is_ok());
    assert!(check(0, 0, 0o775, me, my_group, None).is_err());
    // User-owned components must still be user-only (UPG 0775 allowed).
    assert!(check(me, my_group, 0o755, me, my_group, ns).is_ok());
    assert!(check(me, my_group, 0o775, me, my_group, ns).is_ok());
    assert!(check(me, other, 0o775, me, my_group, ns).is_err());
    assert!(check(me, my_group, 0o1777, me, my_group, ns).is_err());
}

// r4: a store whose git directory this process cannot write leaves its commit
// to the host; a writable one commits as before. trace:BUG-1929 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1929_store_commit_deferred_only_when_git_dir_unwritable() {
    use std::os::unix::fs::PermissionsExt;
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: root ignores directory write bits");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    assert!(!store_commit_deferred_to_host(&store), "host store commits");
    let objects = main.join(".git/objects");
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o555)).unwrap();
    let deferred = store_commit_deferred_to_host(&store);
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(deferred, "an unwritable object database defers to the host");
    assert!(!store_commit_deferred_to_host(&store));
}

// The GIT_ATTR_SOURCE version gate: 2.40+ only; anything unparseable is unsafe.
// trace:BUG-2131 | ai:claude
#[test]
fn bug_2131_git_version_gate() {
    use crate::store_hooks::{attr_source_supported_by as ok, parse_git_version as parse};
    assert_eq!(parse("2.39.5"), Some((2, 39)));
    assert_eq!(parse("git version 2.43.0.windows.1\n"), Some((2, 43)));
    assert_eq!(parse("git version 2.40.1 (Apple Git-143)"), Some((2, 40)));
    assert!(!ok("2.39.5"));
    assert!(!ok("git version 2.39.5"));
    assert!(ok("2.40.0"));
    assert!(ok("2.43.0.windows.1"));
    assert!(ok("git version 2.40.1 (Apple Git-143)"));
    assert!(ok("git version 3.0.0"));
    assert!(!ok("git version 1.99.9"));
    for garbage in [
        "",
        "garbage",
        "git version ",
        "git version x.y",
        "2",
        "2.",
        "v2.45.0",
    ] {
        assert_eq!(parse(garbage), None, "{garbage:?}");
        assert!(!ok(garbage), "{garbage:?} must count as unsafe");
    }
}

// Under a faked pre-2.40 (or unknown) git, every store git path refuses before
// touching the index: the builder is the refusal shim, the write guard names
// the version and the fix, and nothing is committed. Code checkouts are
// unaffected. trace:BUG-2131 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2131_old_git_refuses_store_writes() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            crate::store_hooks::FAKE_GIT_VERSION.with(|f| *f.borrow_mut() = None);
        }
    }
    for (fake, expect) in [
        ("git version 2.39.5", "host git is 2.39.5"),
        ("not a version", "could not be determined"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (main, store) = bug_1929_main_with_store(dir.path());
        let before = git(&store, &["rev-parse", "HEAD"]).unwrap().stdout;
        let _reset = Reset;
        crate::store_hooks::FAKE_GIT_VERSION.with(|f| *f.borrow_mut() = Some(fake.to_string()));
        assert!(!crate::store_hooks::supports_attr_source(&store));

        let safe = format!("{:#}", ensure_store_write_safe(&store).unwrap_err());
        assert!(
            safe.contains(expect) && safe.contains("Upgrade git to 2.40"),
            "{fake}: {safe}"
        );
        assert_eq!(
            store_worktree_issue(&store),
            Some("host git is older than 2.40 and cannot ignore store .gitattributes")
        );

        std::fs::write(store.join("notes.txt"), "planted\n").unwrap();
        let out = host_git_command(&store)
            .args(["add", "notes.txt"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "{fake}: {out:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("ignores GIT_ATTR_SOURCE"),
            "{fake}: {out:?}"
        );
        let committed = crate::db::with_store_write_lock(&store, || {
            add(&store, &["notes.txt"])?;
            commit_paths(&store, "chore: store write", &["notes.txt"])
        });
        assert!(committed.is_err(), "{fake}: store write must refuse");
        crate::store_hooks::FAKE_GIT_VERSION.with(|f| *f.borrow_mut() = None);
        assert_eq!(git(&store, &["rev-parse", "HEAD"]).unwrap().stdout, before);
        let staged = Command::new("git")
            .current_dir(&store)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap();
        assert!(staged.stdout.is_empty(), "{fake}: {staged:?}");

        // A code checkout never depends on the gate.
        crate::store_hooks::FAKE_GIT_VERSION.with(|f| *f.borrow_mut() = Some(fake.to_string()));
        assert!(host_git_command(&main)
            .args(["status", "--porcelain"])
            .output()
            .unwrap()
            .status
            .success());
    }
}
