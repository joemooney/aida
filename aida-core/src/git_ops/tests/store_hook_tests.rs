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
        let staged = crate::test_git::git_command()
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
    assert!(ok(me, my_group, 0o755, me, Some(my_group)));
    assert!(
        ok(me, my_group, 0o775, me, Some(my_group)),
        "user-private group 0775 is owner-only"
    );
    assert!(
        !ok(me, other, 0o775, me, Some(my_group)),
        "a shared group could swap hooks"
    );
    assert!(
        !ok(other, my_group, 0o775, me, Some(my_group)),
        "another owner's group-write"
    );
    assert!(
        !ok(me, my_group, 0o777, me, Some(my_group)),
        "world-writable"
    );
    assert!(
        !ok(me, my_group, 0o757, me, Some(my_group)),
        "world-writable without group-write"
    );
    assert!(
        ok(me, other, 0o755, me, Some(my_group)),
        "no group-write: the group does not matter"
    );
    // TASK-1759: a shared primary group (`users`) is not private, so its
    // group-write refuses even on the user's own directory.
    // trace:TASK-1952 | trace:TASK-1759 | ai:claude
    assert!(
        !ok(me, my_group, 0o775, me, None),
        "a shared primary group could swap hooks"
    );
    assert!(ok(me, my_group, 0o755, me, None));
}

// TASK-1759: only a group named after the user with no supplementary members
// is a user-private group. trace:TASK-1952 | trace:TASK-1759 | ai:claude
#[test]
fn task_1759_user_private_group_rule() {
    use crate::store_hooks::is_user_private_group as upg;
    let me = ["joe".to_string()];
    assert!(upg("joe", "joe", &[], &me));
    assert!(upg("joe", "joe", &[], &[]));
    assert!(!upg("joe", "users", &[], &me), "shared primary group");
    assert!(
        !upg("joe", "joe", &["eve".to_string()], &me),
        "a member could write"
    );
    assert!(!upg("", "", &[], &[]));
    // TASK-2125: another account whose primary gid is the group never shows
    // in gr_mem, but it could write. trace:TASK-1952 | trace:TASK-2125 | ai:claude
    assert!(
        !upg("joe", "joe", &[], &["joe".to_string(), "eve".to_string()]),
        "another account's primary group"
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
    // TASK-1759: 0775 is owner-only only under a user-private primary group
    // (CI runners may run with a shared one); otherwise it refuses.
    // trace:TASK-1952 | trace:TASK-1759 | ai:claude
    // SAFETY: geteuid/getegid have no preconditions.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    if crate::store_hooks::private_primary_gid(uid, gid).is_none() {
        assert!(
            resolve_store_hooks_dir(&store).is_err(),
            "a 0775 checkout under a shared primary group must refuse"
        );
        return;
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
    assert!(check(overflow, overflow, 0o755, me, Some(my_group), ns).is_ok());
    assert!(
        check(overflow, overflow, 0o1777, me, Some(my_group), ns).is_ok(),
        "sticky world-writable like /tmp"
    );
    // Outside a namespace the overflow uid is just another user.
    let err = check(overflow, overflow, 0o755, me, Some(my_group), None).unwrap_err();
    assert!(err.contains("untrusted owner uid 65534"), "{err}");
    // Unsafe modes refuse even inside a namespace.
    let err = check(overflow, overflow, 0o775, me, Some(my_group), ns).unwrap_err();
    assert!(err.contains("group- or world-writable"), "{err}");
    assert!(check(overflow, overflow, 0o757, me, Some(my_group), ns).is_err());
    assert!(check(overflow, overflow, 0o777, me, Some(my_group), ns).is_err());
    // A different overflow value does not admit 65534.
    assert!(check(overflow, overflow, 0o755, me, Some(my_group), Some(4242)).is_err());
    // Other owners always refuse, namespace or not.
    let err = check(other, other, 0o755, me, Some(my_group), ns).unwrap_err();
    assert!(err.contains("untrusted owner uid 2000"), "{err}");
    // Root rule unchanged.
    assert!(check(0, 0, 0o755, me, Some(my_group), None).is_ok());
    assert!(check(0, 0, 0o1777, me, Some(my_group), None).is_ok());
    assert!(check(0, 0, 0o775, me, Some(my_group), None).is_err());
    // User-owned components must still be user-only (UPG 0775 allowed).
    assert!(check(me, my_group, 0o755, me, Some(my_group), ns).is_ok());
    assert!(check(me, my_group, 0o775, me, Some(my_group), ns).is_ok());
    assert!(check(me, other, 0o775, me, Some(my_group), ns).is_err());
    assert!(check(me, my_group, 0o1777, me, Some(my_group), ns).is_err());
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

fn bug_2131_contained_env_set() -> bool {
    std::env::var("AIDA_CONTAINED").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
}

// Old (or unknown) git on a project with no contained writer: store git keeps
// working, carries GIT_ATTR_SOURCE anyway, and the verdict is a warning that
// contained mode is unavailable. trace:BUG-2131 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2131_old_git_uncontained_project_works_with_warning() {
    if bug_2131_contained_env_set() {
        return; // This process is itself contained; the gate must refuse here.
    }
    use crate::store_hooks::{attr_source_verdict, AttrSourceVerdict};
    for fake in ["git version 2.39.5", "not a version"] {
        let dir = tempfile::tempdir().unwrap();
        let (main, store) = bug_1929_main_with_store(dir.path());
        // Explicitly off, so a user-global `contained = true` cannot leak in.
        std::fs::create_dir_all(main.join(".aida")).unwrap();
        std::fs::write(
            main.join(".aida/agents.toml"),
            "[agents]\ncontained = false\n",
        )
        .unwrap();
        with_host_git_version(fake, || {
            match attr_source_verdict(&store) {
                AttrSourceVerdict::WarnOnly(w) => assert!(
                    w.contains("contained sessions are unavailable until git is 2.40"),
                    "{fake}: {w}"
                ),
                other => panic!("{fake}: expected a warning, got {other:?}"),
            }
            ensure_store_write_safe(&store).unwrap();
            assert_eq!(store_worktree_issue(&store), None);
            crate::db::with_store_write_lock(&store, || {
                std::fs::write(store.join("notes.txt"), "one\n")?;
                add(&store, &["notes.txt"])?;
                commit_paths(&store, "chore: store write", &["notes.txt"])
            })
            .unwrap_or_else(|e| panic!("{fake}: {e:#}"));
        });
        let log = git(&store, &["log", "-1", "--format=%s"]).unwrap().stdout;
        assert_eq!(log, "chore: store write", "{fake}");
    }
}

// Old (or unknown) git with contained mode in play, by project config or by a
// contained-launch marker: every store git path refuses before touching the
// index. trace:BUG-2131 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2131_old_git_contained_project_refuses_store_git() {
    for (fake, expect) in [
        ("git version 2.39.5", "host git is 2.39.5"),
        ("not a version", "could not be determined"),
    ] {
        for source in ["config", "agents", "marker"] {
            let dir = tempfile::tempdir().unwrap();
            let (main, store) = bug_1929_main_with_store(dir.path());
            std::fs::create_dir_all(main.join(".aida")).unwrap();
            match source {
                "config" => std::fs::write(
                    main.join(".aida/config.toml"),
                    "[contained]\nenable = true\n",
                )
                .unwrap(),
                "agents" => std::fs::write(
                    main.join(".aida/agents.toml"),
                    "[agents]\ncontained = true\n",
                )
                .unwrap(),
                _ => {
                    std::fs::write(
                        main.join(".aida/agents.toml"),
                        "[agents]\ncontained = false\n",
                    )
                    .unwrap();
                    mark_store_contained(&store).unwrap();
                    assert!(main
                        .join(".git")
                        .join(crate::store_hooks::CONTAINED_MARKER)
                        .is_file());
                }
            }
            let before = git(&store, &["rev-parse", "HEAD"]).unwrap().stdout;
            std::fs::write(store.join("notes.txt"), "planted\n").unwrap();
            with_host_git_version(fake, || {
                let safe = format!("{:#}", ensure_store_write_safe(&store).unwrap_err());
                assert!(
                    safe.contains(expect)
                        && safe.contains("contained sessions are enabled")
                        && safe.contains("Upgrade git to 2.40"),
                    "{fake}/{source}: {safe}"
                );
                assert_eq!(
                    store_worktree_issue(&store),
                    Some("contained sessions are enabled and host git is older than 2.40")
                );
                let out = host_git_command(&store)
                    .args(["add", "notes.txt"])
                    .output()
                    .unwrap();
                assert!(!out.status.success(), "{fake}/{source}: {out:?}");
                assert!(
                    String::from_utf8_lossy(&out.stderr).contains("ignores GIT_ATTR_SOURCE"),
                    "{fake}/{source}: {out:?}"
                );
                let committed = crate::db::with_store_write_lock(&store, || {
                    add(&store, &["notes.txt"])?;
                    commit_paths(&store, "chore: store write", &["notes.txt"])
                });
                assert!(
                    committed.is_err(),
                    "{fake}/{source}: store write must refuse"
                );
                // A code checkout never depends on the gate.
                assert!(host_git_command(&main)
                    .args(["status", "--porcelain"])
                    .output()
                    .unwrap()
                    .status
                    .success());
                // Launching or enabling contained mode refuses too.
                let launch = format!("{:#}", ensure_git_supports_contained().unwrap_err());
                assert!(
                    launch.contains("refusing contained mode") && launch.contains(expect),
                    "{launch}"
                );
            });
            assert_eq!(git(&store, &["rev-parse", "HEAD"]).unwrap().stdout, before);
            let staged = crate::test_git::git_command()
                .current_dir(&store)
                .args(["diff", "--cached", "--name-only"])
                .output()
                .unwrap();
            assert!(staged.stdout.is_empty(), "{fake}/{source}: {staged:?}");
        }
    }
    with_host_git_version("git version 2.40.0", || {
        ensure_git_supports_contained().unwrap();
    });
}

// A config file that exists but cannot be parsed counts as contained.
// trace:BUG-2131 | ai:claude
#[test]
fn bug_2131_contained_config_precedence_fails_closed() {
    use crate::store_hooks::contained_configured_from as on;
    let dir = tempfile::tempdir().unwrap();
    let p = |name: &str, body: Option<&str>| {
        let path = dir.path().join(name);
        if let Some(body) = body {
            std::fs::write(&path, body).unwrap();
        }
        path
    };
    let global_on = p("g-on.toml", Some("[agents]\ncontained = true\n"));
    let agents_off = p("a-off.toml", Some("[agents]\ncontained = false\n"));
    let config_on = p("c-on.toml", Some("[contained]\nenable = true\n"));
    let missing = p("missing.toml", None);
    let broken = p("broken.toml", Some("[agents\ncontained = "));
    assert!(!on(None, &missing, &missing));
    assert!(on(Some(&global_on), &missing, &missing));
    assert!(!on(Some(&global_on), &agents_off, &missing));
    assert!(on(Some(&global_on), &agents_off, &config_on));
    assert!(
        on(None, &broken, &missing),
        "unparseable counts as contained"
    );
}

// The host git version is cached, but not forever: a long-running server
// re-reads it once the cache is older than GIT_VERSION_TTL, so a git upgrade
// or downgrade takes effect without a restart.
// trace:TASK-1775 | ai:claude
#[test]
fn task_1775_git_version_cache_expires() {
    use crate::store_hooks::{cached_git_version, GitVersionCache, GIT_VERSION_TTL};
    let cache: GitVersionCache = std::sync::Mutex::new(None);
    let t0 = std::time::Instant::now();
    let first = cached_git_version(&cache, t0, || "git version 2.39.5".to_string());
    assert_eq!(first, "git version 2.39.5");
    let fresh = cached_git_version(&cache, t0 + GIT_VERSION_TTL / 2, || {
        panic!("a fresh cache must not re-read git")
    });
    assert_eq!(fresh, "git version 2.39.5");
    let reread = cached_git_version(&cache, t0 + GIT_VERSION_TTL, || {
        "git version 2.45.0".to_string()
    });
    assert_eq!(
        reread, "git version 2.45.0",
        "an expired cache re-reads git"
    );
    assert!(crate::store_hooks::attr_source_supported_by(&reread));
}

// BUG-2140: deferral notice names the unwritable path without "sandboxed session",
// `commit_pending_store_changes_on_host` commits deferred writes and loudly
// refuses embedded git repositories, and `auto_commit_paths` stages deferred
// objects alongside `oplog.yaml`.
// trace:BUG-2140 | ai:antigravity
#[cfg(unix)]
#[test]
fn bug_2140_deferral_notice_and_host_commit_and_oplog_coherence() {
    use crate::db::DatabaseBackend;
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipping: root ignores directory write bits");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    let objects = main.join(".git/objects");

    // 1. Simulate a contained / unwritable store git directory and add a requirement.
    let backend = crate::db::GitBackend::new(&store).unwrap();
    let head_before = git(&store, &["rev-parse", "HEAD"]).unwrap().stdout;
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o555)).unwrap();
    let notice = format_store_commit_deferred_notice(&store);
    let r1 = crate::models::Requirement::new(
        "Contained spec".to_string(),
        "written while git dir unwritable".to_string(),
    );
    let added1 = backend.add_requirement(r1).unwrap();
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        notice.contains("cannot write ")
            && notice.contains(": store change queued for host commit")
            && !notice.contains("sandboxed session"),
        "unexpected deferral notice: {notice}"
    );
    assert_eq!(
        git(&store, &["rev-parse", "HEAD"]).unwrap().stdout,
        head_before,
        "deferred write must not commit while git dir is unwritable"
    );

    // 2. Problem 4 regression: a subsequent host-side targeted write to a
    // DIFFERENT spec must stage the prior deferred object alongside `oplog.yaml`,
    // never committing `oplog.yaml` while leaving `added1`'s YAML untracked.
    let r2 = crate::models::Requirement::new(
        "Host spec".to_string(),
        "written on host after contained write".to_string(),
    );
    let added2 = backend.add_requirement(r2).unwrap();
    let rel1 =
        crate::object_store::relative_object_path(added1.spec_id.as_deref().unwrap()).unwrap();
    let rel2 =
        crate::object_store::relative_object_path(added2.spec_id.as_deref().unwrap()).unwrap();
    let committed_files = git(&store, &["show", "--name-only", "--format=", "HEAD"])
        .unwrap()
        .stdout;
    assert!(
        committed_files.lines().any(|l| l == rel1)
            && committed_files.lines().any(|l| l == rel2)
            && committed_files.lines().any(|l| l == "oplog.yaml"),
        "host targeted commit must include prior deferred object + oplog: {committed_files}"
    );

    // 3. Problem 1 regression: another deferred write is committed by
    // `commit_pending_store_changes_on_host`, and an embedded `.git` plant is
    // refused loudly (returns Err).
    let head_mid = git(&store, &["rev-parse", "HEAD"]).unwrap().stdout;
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o555)).unwrap();
    let r3 = crate::models::Requirement::new(
        "Second contained spec".to_string(),
        "committed by host boundary".to_string(),
    );
    let _added3 = backend.add_requirement(r3).unwrap();
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        git(&store, &["rev-parse", "HEAD"]).unwrap().stdout,
        head_mid
    );

    // Plant an embedded `.git` inside the store; host commit must fail loudly.
    let planted_git = store.join("objects/evil/.git");
    std::fs::create_dir_all(&planted_git).unwrap();
    std::fs::write(planted_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let err = commit_pending_store_changes_on_host(&store).unwrap_err();
    assert!(
        format!("{err:#}").contains("embedded git repository"),
        "expected loud embedded-repo refusal, got: {err:#}"
    );
    assert_eq!(
        git(&store, &["rev-parse", "HEAD"]).unwrap().stdout,
        head_mid,
        "refused host commit must not advance HEAD"
    );

    // Remove the plant; now `commit_pending_store_changes_on_host` succeeds and
    // advances HEAD.
    std::fs::remove_dir_all(store.join("objects/evil")).unwrap();
    assert!(commit_pending_store_changes_on_host(&store).unwrap());
    assert_ne!(
        git(&store, &["rev-parse", "HEAD"]).unwrap().stdout,
        head_mid
    );
    assert!(!commit_pending_store_changes_on_host(&store).unwrap());
}

// One store git command resolves the store hooks directory once: the
// `core.hooksPath` pin and the refusal check share the result. Resolving it
// twice spawned an extra `git rev-parse` on every store git call.
// trace:BUG-2192 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2192_host_git_command_resolves_store_hooks_once() {
    let dir = tempfile::tempdir().unwrap();
    let (main, store) = bug_1929_main_with_store(dir.path());
    let resolves = |cwd: &Path| {
        STORE_HOOKS_RESOLVES.with(|n| n.set(0));
        let cmd = host_git_command(cwd);
        (STORE_HOOKS_RESOLVES.with(std::cell::Cell::get), cmd)
    };
    let (n, mut cmd) = resolves(&store);
    assert_eq!(n, 1, "one store git command must resolve the hooks once");
    assert!(cmd.args(["status", "--short"]).status().unwrap().success());
    assert_eq!(
        resolves(&main).0,
        0,
        "a host tree never resolves store hooks"
    );
}

/// Run `f` with every file treated as settled, so the BUG-2192 memo can hit
/// on a fixture made moments ago, and return what it returned plus the lookup
/// git processes it spawned.
// trace:BUG-2192 | ai:claude
fn bug_2192_settled_spawns<T>(f: impl FnOnce() -> T) -> (T, usize) {
    use crate::host_git_cache::{ALWAYS_SETTLED, LOOKUP_SPAWNS};
    ALWAYS_SETTLED.with(|s| s.set(true));
    LOOKUP_SPAWNS.with(|n| n.set(0));
    let out = f();
    ALWAYS_SETTLED.with(|s| s.set(false));
    (out, LOOKUP_SPAWNS.with(std::cell::Cell::get))
}

// BUG-2192: the lookups one store git command needs (the common directory
// and the index/HEAD `.gitmodules`) spawn git once each, then are memoised
// while the files they depend on are unchanged. A changed `.git` pointer or
// index recomputes them, so a `.gitmodules` staged later still gets its
// override.
// trace:BUG-2192.ac421074 | ai:claude
// trace:BUG-2192 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2192_store_git_lookup_spawns_are_memoised_until_the_repo_changes() {
    let dir = tempfile::tempdir().unwrap();
    let (_main, store) = bug_1929_main_with_store(dir.path());
    let args = |cmd: &Command| -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    };

    let (cmd, first) = bug_2192_settled_spawns(|| host_git_command(&store));
    assert_eq!(
        first, 2,
        "first store command: rev-parse + .gitmodules probe"
    );
    let (mut cmd2, second) = bug_2192_settled_spawns(|| host_git_command(&store));
    assert_eq!(second, 0, "an unchanged store reuses both lookups");
    assert_eq!(args(&cmd), args(&cmd2));
    assert!(cmd2.args(["status", "--short"]).status().unwrap().success());

    // Rewriting the `.git` pointer (same text, new inode) re-runs rev-parse.
    let pointer = std::fs::read(store.join(".git")).unwrap();
    std::fs::remove_file(store.join(".git")).unwrap();
    std::fs::write(store.join(".git"), pointer).unwrap();
    let (_, after_pointer) = bug_2192_settled_spawns(|| host_git_command(&store));
    assert_eq!(
        after_pointer, 2,
        "a new .git pointer recomputes both lookups"
    );

    // Stage a .gitmodules (index only): the probe re-runs and the override
    // for its submodule is added.
    let gitmodules = "[submodule \"planted\"]\n\tpath = planted\n\turl = ./x\n";
    std::fs::write(store.join(".gitmodules"), gitmodules).unwrap();
    let blob = run_git(&store, &["hash-object", "-w", ".gitmodules"]);
    std::fs::remove_file(store.join(".gitmodules")).unwrap();
    run_git(
        &store,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{},.gitmodules", blob.trim()),
        ],
    );
    let (cmd, after_index) = bug_2192_settled_spawns(|| host_git_command(&store));
    assert_eq!(after_index, 1, "a changed index re-runs only the probe");
    assert!(
        args(&cmd).contains(&"submodule.planted.ignore=all".to_string()),
        "{:?}",
        args(&cmd)
    );
}

// BUG-2192: nothing is memoised over files changed in the last two seconds,
// since a timestamp tick could hide a rewrite.
// trace:BUG-2192 | ai:claude
#[cfg(unix)]
#[test]
fn bug_2192_recently_changed_store_is_never_memoised() {
    let dir = tempfile::tempdir().unwrap();
    let (_main, store) = bug_1929_main_with_store(dir.path());
    let spawns = || {
        // A fresh `.git` pointer keeps this independent of fixture speed.
        let pointer = std::fs::read(store.join(".git")).unwrap();
        std::fs::remove_file(store.join(".git")).unwrap();
        std::fs::write(store.join(".git"), pointer).unwrap();
        crate::host_git_cache::LOOKUP_SPAWNS.with(|n| n.set(0));
        let _ = host_git_command(&store);
        crate::host_git_cache::LOOKUP_SPAWNS.with(std::cell::Cell::get)
    };
    assert_eq!(spawns(), 2);
    assert_eq!(spawns(), 2, "a just-written store is looked up every time");
}

// TASK-1942: host git over a linked worktree (a contained-writable tree)
// memoises its `.gitmodules` lookup, the file case included: an unchanged
// tree spawns no lookup git, and an edited `.gitmodules` is re-read so its
// new submodule still gets its override.
// trace:TASK-1952 | trace:TASK-1942 | ai:claude
#[cfg(unix)]
#[test]
fn task_1942_linked_worktree_gitmodules_lookup_is_memoised() {
    let dir = tempfile::tempdir().unwrap();
    let (main, _store) = bug_1929_main_with_store(dir.path());
    let wt = dir.path().join("wt");
    run_git(
        &main,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    assert!(is_contained_writable_tree(&wt));
    let args = |cmd: &Command| -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    };

    // No `.gitmodules`: the index/HEAD blob probes run once.
    let (_, first) = bug_2192_settled_spawns(|| host_git_command(&wt));
    assert!(first >= 1, "first command probes .gitmodules");
    let (_, second) = bug_2192_settled_spawns(|| host_git_command(&wt));
    assert_eq!(second, 0, "an unchanged worktree reuses the probe");

    // A `.gitmodules` file: one read, then memoised.
    std::fs::write(
        wt.join(".gitmodules"),
        "[submodule \"one\"]\n\tpath = one\n\turl = ./x\n",
    )
    .unwrap();
    let (_, file_first) = bug_2192_settled_spawns(|| host_git_command(&wt));
    assert_eq!(file_first, 1);
    let (cmd, file_second) = bug_2192_settled_spawns(|| host_git_command(&wt));
    assert_eq!(file_second, 0, "an unchanged .gitmodules is not re-read");
    assert!(args(&cmd).contains(&"submodule.one.ignore=dirty".to_string()));

    // Replacing it re-reads it. TASK-2148: the new content has a different
    // length, so the stamp changes even when the inode is reused within one
    // coarse ctime tick under ALWAYS_SETTLED.
    // trace:TASK-1952 | trace:TASK-2148 | ai:claude
    std::fs::remove_file(wt.join(".gitmodules")).unwrap();
    std::fs::write(
        wt.join(".gitmodules"),
        "[submodule \"second\"]\n\tpath = second\n\turl = ./xyz\n",
    )
    .unwrap();
    let (cmd, after) = bug_2192_settled_spawns(|| host_git_command(&wt));
    assert_eq!(after, 1);
    assert!(args(&cmd).contains(&"submodule.second.ignore=dirty".to_string()));
}

// TASK-1943: an edited submodule whose path git C-quotes in porcelain v1 is
// listed once, and the added line is quoted exactly as git quotes it.
// trace:TASK-1952 | trace:TASK-1943 | ai:claude
#[cfg(unix)]
#[test]
fn task_1943_quoted_porcelain_path_is_listed_once() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("r");
    init(&repo).unwrap();
    let name = "sub d\u{ef}r\t\"x\"";
    std::fs::write(repo.join(name), "x").unwrap();
    let git_line = run_git(&repo, &["status", "--porcelain"]);
    let git_line = git_line.lines().next().unwrap().to_string();
    assert!(git_line.starts_with("?? \""), "git quotes it: {git_line}");
    assert_eq!(porcelain_quote(name), git_line[3..], "quoted like git");
    assert_eq!(porcelain_unquote(&git_line[3..]), name);

    let mut porcelain = format!("{git_line}\n");
    note_edited_submodule(&mut porcelain, name);
    assert_eq!(
        porcelain,
        format!("{git_line}\n"),
        "already listed: no second line"
    );
    let mut empty = String::new();
    note_edited_submodule(&mut empty, name);
    assert_eq!(empty, format!(" M {}\n", &git_line[3..]));
    let mut plain = String::new();
    note_edited_submodule(&mut plain, "plain/sub");
    assert_eq!(plain, " M plain/sub\n");
}

// TASK-2146: the contained safety scan pins `core.quotePath=true`, so a
// non-ASCII path is C-quoted the way `porcelain_quote` quotes the added
// submodule line, even when the operator set `core.quotePath=false`.
// trace:TASK-1952 | trace:TASK-2146 | ai:claude
#[cfg(unix)]
#[test]
fn task_2146_contained_scan_quotes_paths_whatever_the_user_quote_path() {
    let dir = tempfile::tempdir().unwrap();
    let (main, _store) = bug_1929_main_with_store(dir.path());
    let wt = dir.path().join("wt");
    run_git(
        &main,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    run_git(&main, &["config", "core.quotePath", "false"]);
    let name = "d\u{ef}r";
    std::fs::write(wt.join(name), "x").unwrap();
    let lines = worktree_status_porcelain_lines(&wt).unwrap();
    assert_eq!(lines, vec![format!("?? {}", porcelain_quote(name))]);
}

// TASK-2125 (PR-2837 review): the password-database walk itself. Every
// account /etc/passwd lists with this process's primary gid is found (local
// accounts are always enumerable), and a second call is served from the
// per-process memo with the same answer.
// trace:TASK-1952 | trace:TASK-2125 | ai:claude
#[cfg(unix)]
#[test]
fn task_2125_primary_gid_walk_finds_local_accounts() {
    // SAFETY: getgid has no preconditions and cannot fail.
    let gid = unsafe { libc::getgid() };
    let local: Vec<String> = std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() > 3 && f[3] == gid.to_string()).then(|| f[0].to_string())
        })
        .collect();
    let found = crate::store_hooks::accounts_with_primary_gid(gid);
    for name in &local {
        assert!(found.contains(name), "{name} missing from {found:?}");
    }
    assert_eq!(crate::store_hooks::accounts_with_primary_gid(gid), found);
}
