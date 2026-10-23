//! BUG-2169 slice S2: the sandbox boundary of a contained seat's private
//! clone, checked against real paths and, where bubblewrap can make a user
//! namespace, against a real sandbox.
//!
//! Two layers bound a contained seat, and both are exercised:
//!
//! - **Claude Code's sandbox**, modelled from the generated `--settings`:
//!   the whole filesystem read-only, the session's cwd plus every existing
//!   `sandbox.filesystem.allowWrite` entry read-write, then every existing
//!   `denyWrite` entry read-only again, with the settings' `env` applied.
//! - **The `[contained] os_wrap` layer**: the exact argv
//!   [`crate::session::bwrap_confinement_args_inner`] builds from
//!   [`crate::session::os_wrap_rw_paths`] and the seat's `--setenv` pairs.
//!
//! Each attack (sketch §7) runs against the clone layout, which is the
//! launcher's real output, and must FAIL there. The rename-aside, detach,
//! objects and refs attacks also run against a frozen copy of the PR #2694
//! layout (8bc07d82e8: a linked worktree granted `objects/`, `refs/`, `logs/`
//! and its gitdir, with file-level denies), where they must SUCCEED. That
//! proves the harness detects the bypass at all.
//!
//! Gating: the `bug_2169_bwrap_` tests need bubblewrap and an unprivileged
//! user namespace. Without them each prints `SKIP (bwrap unavailable)` and
//! passes, unless `AIDA_REQUIRE_BWRAP_LIVE=1` is set (the CI gate), where a
//! skip fails the test.
//! trace:BUG-2169 | ai:claude

// Bubblewrap, and both sandbox layers modelled here, are Linux-only.
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git_ok(dir: &Path, args: &[&str]) -> String {
        // trace:BUG-2169 | ai:claude
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

    /// Whether the live bubblewrap tests can run here. Fails the test instead of
    /// skipping when `AIDA_REQUIRE_BWRAP_LIVE` is set.
    fn bwrap_live() -> bool {
        let ok = Command::new("bwrap")
            .args(["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"])
            .arg("true")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            assert!(
                std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
                "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot create an unprivileged user namespace \
             here, so the BUG-2169 sandbox gate could not run. Install bubblewrap and/or \
             `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`."
            );
            eprintln!("SKIP (bwrap unavailable)");
        }
        ok
    }

    /// A directory for fixtures OUTSIDE `/tmp`: the os_wrap layer mounts a fresh
    /// tmpfs on `/tmp`, which would hide a fixture living there. Uses the
    /// workspace's (gitignored) `target/`.
    fn fixture_tempdir() -> tempfile::TempDir {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let base = Some(workspace.join("target"))
            .filter(|p| !p.starts_with("/tmp"))
            .unwrap_or_else(std::env::temp_dir)
            .join("bug-2169-bwrap");
        std::fs::create_dir_all(&base).unwrap();
        tempfile::Builder::new()
            .prefix("fx-")
            .tempdir_in(base)
            .unwrap()
    }

    /// Give the fixture its own store and stop every store walk-up at the
    /// fixture root. The fixture lives under the workspace's `target/`, so a
    /// walk from a config naming no store (the seat configs below) used to
    /// climb to this checkout's `.aida/config.toml` and resolve the
    /// operator's real store, where the seat grants then wrote the hermetic
    /// test user's roster (BUG-2209).
    // trace:BUG-2209 | ai:claude
    fn seal_store_walk(root: &Path, main: &Path) {
        std::fs::create_dir_all(main.join(".aida-store/objects")).unwrap();
        std::fs::create_dir_all(main.join(".aida-store/registry")).unwrap();
        let store = main.join(".aida-store");
        std::fs::write(store.join("objects/.keep"), "").unwrap();
        git_ok(&store, &["init", "-q", "-b", "aida-store"]);
        git_ok(&store, &["config", "user.email", "test@example.com"]);
        git_ok(&store, &["config", "user.name", "AIDA Test"]);
        git_ok(&store, &["add", "."]);
        git_ok(&store, &["commit", "-qm", "store seed"]);
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        let rel = main.strip_prefix(root).unwrap().join(".aida-store");
        std::fs::write(
            root.join(".aida/config.toml"),
            format!(
                "store_path = {}\n",
                aida_core::toml_quote::toml_string(&rel.display().to_string())
            ),
        )
        .unwrap();
    }

    /// BUG-2209: a store walk from anywhere in the fixture, under the seat
    /// configs that name no store, resolves the fixture's own store, and the
    /// seat grant writes its roster there.
    // trace:BUG-2209 | ai:claude
    #[test]
    fn bug_2209_fixture_store_walk_stays_in_the_fixture() {
        let fx = fixture();
        std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
        std::fs::write(
            fx.main.join(".aida/config.toml"),
            "[contained]\nseat_tree = \"clone\"\n",
        )
        .unwrap();
        let seats = fx.root.join("seats/clone");
        std::fs::create_dir_all(&seats).unwrap();
        let store = fx.main.join(".aida-store").canonicalize().unwrap();
        for start in [&fx.main, &fx.other_wt, &seats] {
            let found =
                crate::detect_distributed_store_from(start).and_then(|s| s.canonicalize().ok());
            assert_eq!(
                found.as_ref(),
                Some(&store),
                "walk from {}",
                start.display()
            );
        }
        drop(ambient(&fx.main));
        let roster = std::fs::read_to_string(store.join("registry/team.toml")).unwrap();
        assert!(
            roster.contains(crate::test_env::HERMETIC_TEST_USER),
            "{roster}"
        );
    }

    /// A main checkout like the real one, plus the attacker's chosen commit.
    ///
    /// - `main` is packed, and `refs/remotes/origin/HEAD` points at `origin/main`
    /// - tag `v0.0.1`, objects in a pack, a `target/` directory
    /// - one other linked worktree on branch `other`
    /// - a bare file:// `origin` it pushed to
    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        main: PathBuf,
        origin: PathBuf,
        other_wt: PathBuf,
        /// A commit in main's objects that `main` does not point at.
        evil: String,
    }

    fn fixture() -> Fixture {
        let tmp = fixture_tempdir();
        let root = tmp.path().canonicalize().unwrap();
        let main = root.join("aida");
        std::fs::create_dir_all(&main).unwrap();
        git_ok(&main, &["init", "-q", "-b", "main"]);
        git_ok(&main, &["config", "user.email", "test@example.com"]);
        git_ok(&main, &["config", "user.name", "AIDA Test"]);
        std::fs::write(main.join("README.md"), "seed\n").unwrap();
        std::fs::write(main.join(".gitignore"), "/target\n/.aida/\n/.aida-store/\n").unwrap();
        git_ok(&main, &["add", "README.md", ".gitignore"]);
        git_ok(&main, &["commit", "-qm", "seed"]);
        git_ok(&main, &["tag", "v0.0.1"]);
        git_ok(&main, &["checkout", "-qb", "evil"]);
        std::fs::write(main.join("EVIL"), "evil\n").unwrap();
        git_ok(&main, &["add", "EVIL"]);
        git_ok(&main, &["commit", "-qm", "evil"]);
        let evil = git_ok(&main, &["rev-parse", "HEAD"]);
        git_ok(&main, &["checkout", "-q", "main"]);
        git_ok(&main, &["branch", "-qD", "evil"]);
        // Where the host itself later moves `main` (T2), kept reachable.
        git_ok(&main, &["commit", "-q", "--allow-empty", "-m", "next"]);
        git_ok(&main, &["update-ref", "refs/keep/next", "HEAD"]);
        git_ok(&main, &["reset", "-q", "--hard", "HEAD~1"]);
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
        git_ok(
            &main,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        let other_wt = root.join("wt-other");
        git_ok(
            &main,
            &[
                "worktree",
                "add",
                "-qb",
                "other",
                other_wt.to_str().unwrap(),
            ],
        );
        git_ok(&main, &["repack", "-qad"]);
        git_ok(&main, &["pack-refs", "--all"]);
        std::fs::create_dir_all(main.join("target")).unwrap();
        seal_store_walk(&root, &main);
        Fixture {
            _tmp: tmp,
            root,
            main,
            origin,
            other_wt,
            evil,
        }
    }

    impl Fixture {
        fn rev(&self, name: &str) -> String {
            git_ok(&self.main, &["rev-parse", name])
        }

        /// Main's refs, as the host sees them.
        fn host_refs(&self) -> String {
            git_ok(
                &self.main,
                &["for-each-ref", "--format=%(refname) %(objectname)"],
            )
        }

        fn pack_count(&self) -> usize {
            std::fs::read_dir(self.main.join(".git/objects/pack"))
                .unwrap()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "pack"))
                .count()
        }
    }

    /// Configure `main` for clone seats under `seats/`, then start a contained
    /// seat exactly as `queue work` / `agent new` do. Returns the clone root.
    fn launch_clone_seat(fx: &Fixture) -> PathBuf {
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
        let _ambient = ambient(&fx.main);
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
        crate::list_leases(&fx.main)
            .into_iter()
            .find(|l| l.scope == "TASK-2169")
            .expect("clone lease")
            .worktree_path
    }

    fn ambient(main: &Path) -> crate::test_env::AmbientGuard {
        crate::test_env::AmbientGuard::hermetic_with_seat_and(
            main,
            "implementer",
            &[],
            &[("AIDA_OS_WRAP", None)],
        )
    }

    /// The contained `--settings` document the launcher gives a seat in `tree`.
    fn settings_for(main: &Path, tree: &Path) -> serde_json::Value {
        let _ambient = ambient(main);
        let roots = crate::session::contained_session_write_roots_from(tree);
        let flags = crate::session::claude_contained_flags_for_roots(&roots).unwrap();
        serde_json::from_str(flags.last().unwrap()).unwrap()
    }

    fn entries(settings: &serde_json::Value, key: &str) -> Vec<PathBuf> {
        settings
            .pointer(&format!("/sandbox/filesystem/{key}"))
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    // `//abs` is Claude Code's absolute anchor.
                    .map(|s| PathBuf::from(s.strip_prefix('/').unwrap_or(s)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A bubblewrap argv modelling Claude Code's Linux sandbox for `settings`
    /// with the session's cwd at `cwd`.
    fn claude_sandbox_argv(settings: &serde_json::Value, cwd: &Path) -> Vec<String> {
        let s = |p: &Path| p.display().to_string();
        // Claude Code's sandbox gives the session a private writable temp dir.
        let mut argv: Vec<String> = [
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
        ]
        .map(String::from)
        .to_vec();
        argv.extend(["--bind".into(), s(cwd), s(cwd)]);
        for path in entries(settings, "allowWrite") {
            if path.exists() {
                argv.extend(["--bind".into(), s(&path), s(&path)]);
            }
        }
        for path in entries(settings, "denyWrite") {
            if path.exists() {
                argv.extend(["--ro-bind".into(), s(&path), s(&path)]);
            }
        }
        if let Some(env) = settings.get("env").and_then(|e| e.as_object()) {
            for (key, value) in env {
                argv.extend([
                    "--setenv".into(),
                    key.clone(),
                    value.as_str().unwrap_or_default().to_string(),
                ]);
            }
        }
        argv.extend(["--chdir".into(), s(cwd), "--die-with-parent".into()]);
        argv
    }

    /// The `[contained] os_wrap` argv for a seat running in `tree`.
    /// Built by the production wrapper itself, `os_wrapped_program_and_args`,
    /// with os_wrap on; the trailing program is dropped so a test supplies its own.
    fn os_wrap_argv(main: &Path, tree: &Path) -> Vec<String> {
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            main,
            "implementer",
            &[],
            &[("AIDA_OS_WRAP", Some("1"))],
        );
        let (program, mut argv) =
            crate::session::os_wrapped_program_and_args(tree, "true", Vec::new()).unwrap();
        assert_eq!(program, "bwrap");
        assert_eq!(argv.pop().as_deref(), Some("true"));
        argv
    }

    /// The PR #2694 layout, frozen from 8bc07d82e8, for the seat in the linked
    /// worktree `wt`: allow the worktree, its gitdir and the common `objects/`,
    /// `refs/`, `logs/`; deny the common `config`, `hooks`, `info`, the gitdir's
    /// pointers, the worktree's `.git` file, and main's materialized loose ref
    /// and reflog.
    fn legacy_2694_settings(fx: &Fixture, wt: &Path) -> serde_json::Value {
        let common = fx.main.join(".git");
        let name = wt.file_name().unwrap();
        let gitdir = common.join("worktrees").join(name);
        let sha = fx.rev("refs/heads/main");
        // Materialize the packed default branch so its deny binds an existing file.
        std::fs::write(common.join("refs/heads/main"), format!("{sha}\n")).unwrap();
        let log = common.join("logs/refs/heads/main");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        if !log.exists() {
            std::fs::write(&log, "").unwrap();
        }
        let allow = [
            wt.to_path_buf(),
            gitdir.clone(),
            common.join("objects"),
            common.join("refs"),
            common.join("logs"),
        ];
        let deny = [
            common.join("config"),
            common.join("hooks"),
            common.join("info"),
            gitdir.join("config.worktree"),
            gitdir.join("commondir"),
            gitdir.join("gitdir"),
            wt.join(".git"),
            common.join("refs/heads/main"),
            log,
        ];
        let spell = |paths: &[PathBuf]| -> Vec<String> {
            paths.iter().map(|p| format!("/{}", p.display())).collect()
        };
        serde_json::json!({
            "sandbox": { "filesystem": { "allowWrite": spell(&allow), "denyWrite": spell(&deny) } }
        })
    }

    struct Ran {
        ok: bool,
        out: String,
    }

    /// Run `script` with `sh -c` inside bubblewrap built from `argv`.
    fn run_in(argv: &[String], script: &str, env: &[(&str, &Path)]) -> Ran {
        let mut cmd = Command::new("bwrap");
        cmd.args(argv).args(["sh", "-c", script]);
        for (key, value) in env {
            cmd.env(key, value);
        }
        seat_git_env(&mut cmd);
        // Both layers mount a fresh `/tmp`; an inherited TMPDIR elsewhere (a
        // `/dev/shm` build-slot dir) does not exist inside.
        cmd.env("TMPDIR", "/tmp");
        let out = cmd.output().expect("spawn bwrap");
        Ran {
            ok: out.status.success(),
            out: format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        }
    }

    /// A seat's git identity, without touching the host's config.
    fn seat_git_env(cmd: &mut Command) {
        cmd.env("GIT_AUTHOR_NAME", "Seat")
            .env("GIT_AUTHOR_EMAIL", "seat@example.com")
            .env("GIT_COMMITTER_NAME", "Seat")
            .env("GIT_COMMITTER_EMAIL", "seat@example.com");
    }

    /// Both layers of a clone seat: Claude Code's sandbox and os_wrap.
    fn clone_layers(fx: &Fixture, clone: &Path) -> Vec<(&'static str, Vec<String>)> {
        vec![
            (
                "claude sandbox",
                claude_sandbox_argv(&settings_for(&fx.main, clone), clone),
            ),
            ("os_wrap", os_wrap_argv(&fx.main, clone)),
        ]
    }

    // ───────────────────────── host trust and settings (no sandbox) ─────────────

    /// The seat's own `.git` is exempt from the BUG-2012 main-checkout deny only
    /// because the HOST's lease vouches for the clone; main's `.git` stays denied.
    #[test]
    fn a_host_recorded_clone_keeps_its_own_git_writable_and_main_git_denied() {
        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let recorded = crate::seat_clone::host_recorded_clone(&clone).expect("host-recorded clone");
        assert_eq!(recorded.main, fx.main);
        let target = recorded.cargo_target_dir.clone().expect("target dir");
        assert_eq!(
            target,
            fx.main
                .join("target/worktrees")
                .join(clone.file_name().unwrap())
        );
        assert!(
            target.is_dir(),
            "session_start must create the seat's target dir"
        );

        let denied =
            crate::contained_boundary::contained_main_checkout_git_deny_paths(&[clone.clone()]);
        assert!(!denied.contains(&clone.join(".git")), "{denied:?}");

        let settings = settings_for(&fx.main, &clone);
        let deny = entries(&settings, "denyWrite");
        let allow = entries(&settings, "allowWrite");
        assert!(!deny.contains(&clone.join(".git")), "{settings}");
        assert!(deny.contains(&fx.main.join(".git")), "{settings}");
        assert!(deny.contains(&fx.main.join(".aida/sessions")), "{settings}");
        let tool_deny = settings["permissions"]["deny"].to_string();
        assert!(
            tool_deny.contains(&format!("Write(/{}/.aida/sessions/**)", clone.display())),
            "{settings}"
        );
        assert!(allow.contains(&target), "{settings}");
        assert!(
            !allow
                .iter()
                .any(|p| p.to_string_lossy().contains("sccache")),
            "ruling O2: no shared compile cache: {settings}"
        );
        assert_eq!(settings["env"]["RUSTC_WRAPPER"], "", "{settings}");
        assert_eq!(
            settings["env"]["CARGO_TARGET_DIR"],
            target.display().to_string(),
            "{settings}"
        );
        let allow_rules = settings["permissions"]["allow"].to_string();
        assert!(allow_rules.contains(&format!("Edit(/{}/**)", clone.display())));
        assert!(!allow_rules.contains(&format!("Edit(/{}/**)", fx.main.display())));
    }

    /// Nothing the seat can write makes a directory a host-recorded clone: not
    /// the `aida-seat-*` name plus marker on a main checkout, not a lease naming
    /// the main checkout itself, and not a directory inside a real clone.
    #[test]
    fn only_the_host_lease_makes_a_host_recorded_clone() {
        let fx = fixture();
        let clone = launch_clone_seat(&fx);

        // A directory inside the clone is not the clone root.
        let sub = clone.join("sub");
        std::fs::create_dir_all(sub.join(".git")).unwrap();
        assert!(crate::seat_clone::host_recorded_clone(&sub).is_none());
        let denied =
            crate::contained_boundary::contained_main_checkout_git_deny_paths(&[sub.clone()]);
        assert!(denied.contains(&sub.join(".git")), "{denied:?}");

        // A forged clone: right name, marker, alternates, but no host lease.
        let forged = fx.root.join("seats/aida-seat-forged");
        std::fs::create_dir_all(forged.join(".git/objects/info")).unwrap();
        std::fs::write(
            forged.join(".git/objects/info/alternates"),
            format!("{}\n", fx.main.join(".git/objects").display()),
        )
        .unwrap();
        let marker = forged.join(crate::seat_clone::MARKER_RELATIVE);
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::copy(clone.join(crate::seat_clone::MARKER_RELATIVE), &marker).unwrap();
        assert!(crate::seat_clone::host_recorded_clone(&forged).is_none());
        let denied =
            crate::contained_boundary::contained_main_checkout_git_deny_paths(&[forged.clone()]);
        assert!(denied.contains(&forged.join(".git")), "{denied:?}");

        // A `clone` lease naming the main checkout itself never exempts it.
        let lease = crate::lease_path(&fx.main, "b2169f0e9ed1");
        std::fs::write(
            lease,
            format!(
                "id = \"b2169f0e9ed1\"\nscope = \"BUG-2169\"\nslug = \"x\"\nowner = \"t\"\n\
             worktree_path = {}\nbranch = \"x\"\nstarted_at = \"2026-10-20T00:00:00Z\"\n\
             hostname = \"h\"\ntree_kind = \"clone\"\n",
                aida_core::toml_quote::toml_string(&fx.main.display().to_string())
            ),
        )
        .unwrap();
        assert!(crate::seat_clone::host_recorded_clone(&fx.main).is_none());
        let denied =
            crate::contained_boundary::contained_main_checkout_git_deny_paths(&[fx.main.clone()]);
        assert!(denied.contains(&fx.main.join(".git")), "{denied:?}");

        // A FORGED MAIN: the seat builds a repository in a directory it can
        // write (its own cargo target dir), points its marker and alternates
        // at it and plants a `clone` lease there. main_for accepts it, since
        // every file it reads is the seat's, but no host process resolved that
        // main, so nothing is granted from it.
        let recorded = crate::seat_clone::host_recorded_clone(&clone).expect("real clone");
        let target = recorded.cargo_target_dir.clone().expect("target dir");
        let fake = target.join("fake-main");
        std::fs::create_dir_all(&fake).unwrap();
        git_ok(&fake, &["init", "-q"]);
        let marker_path = clone.join(crate::seat_clone::MARKER_RELATIVE);
        let marker = std::fs::read_to_string(&marker_path).unwrap();
        let forged_marker = marker.replace(
            &format!(
                "main = {}",
                aida_core::toml_quote::toml_string(&fx.main.display().to_string())
            ),
            &format!(
                "main = {}",
                aida_core::toml_quote::toml_string(&fake.display().to_string())
            ),
        );
        assert_ne!(forged_marker, marker, "the marker names main: {marker}");
        std::fs::write(&marker_path, forged_marker).unwrap();
        let alternates = clone.join(".git/objects/info/alternates");
        let real_alternates = std::fs::read_to_string(&alternates).unwrap();
        std::fs::write(
            &alternates,
            format!("{}\n", fake.join(".git/objects").display()),
        )
        .unwrap();
        let elsewhere = fx
            .root
            .join("elsewhere/worktrees")
            .join(clone.file_name().unwrap());
        std::fs::create_dir_all(&elsewhere).unwrap();
        let forged_lease = crate::lease_path(&fake, "b2169f0e9ed2");
        std::fs::create_dir_all(forged_lease.parent().unwrap()).unwrap();
        std::fs::write(
            forged_lease,
            format!(
                "id = \"b2169f0e9ed2\"\nscope = \"BUG-2169\"\nslug = \"x\"\nowner = \"t\"\n\
                 worktree_path = {}\nbranch = \"x\"\nstarted_at = \"2026-10-20T00:00:00Z\"\n\
                 hostname = \"h\"\ntree_kind = \"clone\"\ncargo_target_dir = {}\n",
                aida_core::toml_quote::toml_string(&clone.display().to_string()),
                aida_core::toml_quote::toml_string(&elsewhere.display().to_string())
            ),
        )
        .unwrap();
        assert_eq!(
            crate::seat_clone::main_for(&clone),
            crate::seat_clone::MainFor::Verified(fake.canonicalize().unwrap()),
            "the forgery satisfies every seat-writable check"
        );
        assert!(
            crate::seat_clone::host_recorded_clone(&clone).is_none(),
            "a forged main is never host-recorded"
        );
        // TASK-2111: a tree whose `.git` directory resolves to a main other
        // than itself, and is not a host-recorded clone, refuses the
        // contained launch outright, so nothing is granted from the forgery.
        // trace:TASK-1952 | trace:TASK-2111 | ai:claude
        let refused = {
            let _ambient = ambient(&fx.main);
            let roots = crate::session::contained_session_write_roots_from(&clone);
            crate::session::claude_contained_flags_for_roots(&roots)
        };
        match refused {
            Err(e) => assert!(
                format!("{e:#}").contains("not a seat clone the host recorded"),
                "{e:#}"
            ),
            Ok(flags) => {
                let settings: serde_json::Value =
                    serde_json::from_str(flags.last().unwrap()).unwrap();
                assert!(
                    !entries(&settings, "allowWrite").contains(&elsewhere),
                    "{settings}"
                );
            }
        }
        assert!(
            !crate::session::os_wrap_rw_paths(&clone).contains(&elsewhere),
            "os_wrap binds nothing from the forged lease"
        );
        std::fs::write(&marker_path, &marker).unwrap();
        std::fs::write(&alternates, real_alternates).unwrap();
        assert!(crate::seat_clone::host_recorded_clone(&clone).is_some());

        // A target dir that is a symlink grants nothing, nor does a lease that
        // records any other path for it.
        std::fs::remove_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &target).unwrap();
        let recorded = crate::seat_clone::host_recorded_clone(&clone).expect("still a clone");
        assert_eq!(
            recorded.cargo_target_dir, None,
            "a symlinked target dir is refused"
        );
        let settings = settings_for(&fx.main, &clone);
        assert!(
            !entries(&settings, "allowWrite").contains(&target),
            "{settings}"
        );
        assert!(
            !entries(&settings, "allowWrite").contains(&elsewhere),
            "{settings}"
        );
    }

    /// Pre-existing defect (sketch §1, signoff condition 2): `agent new`
    /// resolved the contained posture from the LAUNCHER's git root, so a seat
    /// launched from the main checkout got main as its write boundary. The
    /// settings are rebuilt for the tree the plan names.
    #[test]
    fn agent_new_scopes_the_contained_boundary_to_the_seat_tree() {
        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let _ambient = ambient(&fx.main);
        let launcher = crate::session::claude_contained_flags_for(&fx.main).unwrap();
        let mut config = crate::agent_launch::AgentLaunchConfig {
            agent_type: "claude",
            binary: "claude",
            default_args: launcher.clone(),
            prompt_style: crate::agent_launch::AgentPromptStyle::Positional,
            contained: true,
        };
        let main_rule = format!("Edit(/{}/**)", fx.main.display());
        assert!(launcher.last().unwrap().contains(&main_rule));

        crate::agent_launch::rescope_contained_settings(&mut config, &clone).unwrap();
        assert_eq!(config.default_args.len(), launcher.len());
        let settings = config.default_args.last().unwrap();
        assert!(!settings.contains(&main_rule), "{settings}");
        assert!(
            settings.contains(&format!("Edit(/{}/**)", clone.display())),
            "{settings}"
        );

        // An uncontained launch is left exactly as it was.
        let mut plain = crate::agent_launch::AgentLaunchConfig {
            contained: false,
            default_args: launcher.clone(),
            ..config
        };
        crate::agent_launch::rescope_contained_settings(&mut plain, &clone).unwrap();
        assert_eq!(plain.default_args, launcher);
    }

    /// Pre-existing defect: `--bg` never applied os_wrap, because Claude Code's
    /// background supervisor starts the session itself. Under os_wrap a
    /// contained `--bg` launch is refused rather than run outside the boundary.
    #[test]
    fn a_contained_bg_launch_is_refused_under_os_wrap() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(tree.join(".aida")).unwrap();
        let config = |contained| crate::agent_launch::AgentLaunchConfig {
            agent_type: "claude",
            binary: "claude",
            default_args: Vec::new(),
            prompt_style: crate::agent_launch::AgentPromptStyle::Positional,
            contained,
        };
        let _ambient = ambient(&tree);
        crate::agent_launch::refuse_bg_under_os_wrap(&config(true), &tree).unwrap();
        std::fs::write(
            tree.join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();
        let err = crate::agent_launch::refuse_bg_under_os_wrap(&config(true), &tree).unwrap_err();
        assert!(
            format!("{err:#}").contains("background supervisor"),
            "{err:#}"
        );
        crate::agent_launch::refuse_bg_under_os_wrap(&config(false), &tree).unwrap();
    }

    /// Pre-existing defect: the interactive `claude` launch never applied
    /// os_wrap. A contained one now goes through the same wrapper; an
    /// uncontained one is unchanged.
    #[test]
    fn a_contained_interactive_launch_goes_through_os_wrap() {
        let fx = fixture();
        std::fs::create_dir_all(fx.other_wt.join(".aida")).unwrap();
        std::fs::write(
            fx.other_wt.join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();
        let _ambient = ambient(&fx.main);
        let _cwd = crate::session::contained_cwd_seam::pin(&fx.other_wt);
        let args = vec!["--name".to_string(), "seat".to_string()];
        let (program, plain) =
            crate::session::interactive_claude_program_and_args(false, args.clone()).unwrap();
        assert_eq!((program.as_str(), &plain), ("claude", &args));
        match crate::session::interactive_claude_program_and_args(true, args.clone()) {
            Ok((program, wrapped)) => {
                assert_eq!(program, "bwrap");
                let at = wrapped
                    .iter()
                    .position(|a| a == "claude")
                    .expect("claude in argv");
                assert_eq!(&wrapped[at + 1..], &args[..]);
                assert!(wrapped
                    .windows(3)
                    .any(|w| w[0] == "--bind" && Path::new(&w[1]) == fx.other_wt));
            }
            // No usable bwrap here: the launch must refuse, never run unwrapped.
            Err(err) => assert!(format!("{err:#}").contains("Refusing"), "{err:#}"),
        }
    }

    // ───────────────────────── real-bwrap attacks (sketch §7) ───────────────────

    /// T3 positive: inside both layers the seat commits, fetches, merges
    /// origin/main and pushes its branch. Main's local refs never change. The
    /// origin stands in for the network remote, so it is bound read-write.
    #[test]
    fn bug_2169_bwrap_t3_clone_seat_commits_fetches_merges_and_pushes() {
        if !bwrap_live() {
            return;
        }
        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let before = fx.host_refs();
        for (layer, mut argv) in clone_layers(&fx, &clone) {
            let at = argv.iter().position(|a| a == "--chdir").unwrap();
            let origin = fx.origin.display().to_string();
            argv.splice(at..at, ["--bind".to_string(), origin.clone(), origin]);
            let branch = format!("seat-{}", layer.replace(' ', "-"));
            let script = format!(
                "set -e; git checkout -qb {branch}; echo {branch} > work.txt; git add work.txt; \
             git -c maintenance.auto=false -c gc.auto=0 commit -qm work; \
             git -c maintenance.auto=false -c gc.auto=0 fetch -q origin; \
             git -c maintenance.auto=false -c gc.auto=0 merge -q --no-edit origin/main; \
             git push -q -u origin {branch}; git rev-parse HEAD"
            );
            let ran = run_in(&argv, &script, &[]);
            assert!(ran.ok, "{layer}: {}", ran.out);
            let head = ran.out.lines().last().unwrap().to_string();
            git_ok(&fx.main, &["fetch", "-q", "origin"]);
            assert_eq!(
                fx.rev(&format!("refs/remotes/origin/{branch}")),
                head,
                "{layer}"
            );
        }
        let after: Vec<String> = fx
            .host_refs()
            .lines()
            .filter(|l| !l.starts_with("refs/remotes/origin/seat-"))
            .map(String::from)
            .collect();
        assert_eq!(after.join("\n"), before, "main's own refs never change");
    }

    /// T1 rename-aside: move `refs/heads` aside so a file-level deny no longer
    /// covers `refs/heads/main`, then write it.
    #[test]
    fn bug_2169_bwrap_t1_rename_aside() {
        if !bwrap_live() {
            return;
        }
        let script = r#"cd "$MAIN/.git/refs" || exit 9
mv heads h.x; mkdir -p heads; mv h.x/* heads/ 2>/dev/null
echo "$EVIL" > heads/main && echo WROTE"#;
        let legacy = fixture();
        let settings = legacy_2694_settings(&legacy, &legacy.other_wt);
        let ran = run_in(
            &claude_sandbox_argv(&settings, &legacy.other_wt),
            &format!(
                "MAIN={} EVIL={}; {script}",
                legacy.main.display(),
                legacy.evil
            ),
            &[],
        );
        assert_eq!(
            legacy.rev("refs/heads/main"),
            legacy.evil,
            "the harness must reproduce the #2694 bypass: {}",
            ran.out
        );

        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let main_before = fx.rev("refs/heads/main");
        for (layer, argv) in clone_layers(&fx, &clone) {
            let ran = run_in(
                &argv,
                &format!("MAIN={} EVIL={}; {script}", fx.main.display(), fx.evil),
                &[],
            );
            assert!(!ran.out.contains("WROTE"), "{layer}: {}", ran.out);
            assert!(
                ran.out.contains("Read-only file system"),
                "{layer}: {}",
                ran.out
            );
            assert_eq!(fx.rev("refs/heads/main"), main_before, "{layer}");
            assert!(fx.main.join(".git/refs/heads").is_dir(), "{layer}");
        }
    }

    /// T2 host detach: while the seat waits, the host replaces `refs/heads/main`
    /// (a lock-then-rename `update-ref`, or `pack-refs --all --prune`), which
    /// detaches a file bind from the path. Then the seat writes the ref.
    #[test]
    fn bug_2169_bwrap_t2_host_detach() {
        if !bwrap_live() {
            return;
        }
        let script = r#"touch "$READY"
i=0; while [ ! -e "$GO" ]; do sleep 0.05; i=$((i+1)); [ $i -gt 600 ] && exit 7; done
echo "$EVIL" > "$MAIN/.git/refs/heads/main" && echo WROTE"#;
        let host_moves: [(&str, &[&str]); 2] = [
            (
                "update-ref",
                &["update-ref", "refs/heads/main", "refs/keep/next"],
            ),
            ("pack-refs", &["pack-refs", "--all", "--prune"]),
        ];
        for (variant, host_cmd) in host_moves {
            let legacy = fixture();
            let settings = legacy_2694_settings(&legacy, &legacy.other_wt);
            let argv = claude_sandbox_argv(&settings, &legacy.other_wt);
            let out = detach_attack(&legacy, &argv, &legacy.other_wt, host_cmd, script);
            assert_eq!(
                legacy.rev("refs/heads/main"),
                legacy.evil,
                "{variant}: the harness must reproduce the #2694 detach bypass: {out}"
            );

            let fx = fixture();
            let clone = launch_clone_seat(&fx);
            // Only the host's own move lands.
            let host_value = match variant {
                "update-ref" => fx.rev("refs/keep/next"),
                _ => fx.rev("refs/heads/main"),
            };
            for (layer, argv) in clone_layers(&fx, &clone) {
                let out = detach_attack(&fx, &argv, &clone, host_cmd, script);
                assert!(!out.contains("WROTE"), "{variant}/{layer}: {out}");
                assert!(
                    out.contains("Read-only file system"),
                    "{variant}/{layer}: {out}"
                );
                assert_eq!(fx.rev("refs/heads/main"), host_value, "{variant}/{layer}");
            }
        }
    }

    /// Start `script` in the sandbox, wait until it is inside, run `host_cmd` in
    /// main on the host, then let the seat continue. Returns the seat's output.
    fn detach_attack(
        fx: &Fixture,
        argv: &[String],
        seat_dir: &Path,
        host_cmd: &[&str],
        script: &str,
    ) -> String {
        let ready = seat_dir.join(".bug2169-ready");
        let go = fx.root.join("go");
        let _ = std::fs::remove_file(&ready);
        let _ = std::fs::remove_file(&go);
        let mut cmd = Command::new("bwrap");
        cmd.args(argv)
            .args(["sh", "-c", script])
            .env("READY", &ready)
            .env("GO", &go)
            .env("MAIN", &fx.main)
            .env("EVIL", &fx.evil)
            .env("TMPDIR", "/tmp")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let child = cmd.spawn().expect("spawn bwrap");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !ready.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the seat never became ready"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        git_ok(&fx.main, host_cmd);
        std::fs::write(&go, "").unwrap();
        let out = child.wait_with_output().unwrap();
        let _ = std::fs::remove_file(&ready);
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    /// T4 objects and alternates: plant and delete packs in main's shared
    /// object store, and repoint the seat's own alternates.
    #[test]
    fn bug_2169_bwrap_t4_objects_and_alternates() {
        if !bwrap_live() {
            return;
        }
        let script = r#"touch "$MAIN/.git/objects/pack/planted" && echo PLANTED
rm -f "$MAIN"/.git/objects/pack/*.pack && echo DELETED
true"#;
        let legacy = fixture();
        let settings = legacy_2694_settings(&legacy, &legacy.other_wt);
        let ran = run_in(
            &claude_sandbox_argv(&settings, &legacy.other_wt),
            script,
            &[("MAIN", &legacy.main)],
        );
        assert!(
            ran.out.contains("PLANTED") && ran.out.contains("DELETED"),
            "{}",
            ran.out
        );
        assert_eq!(
            legacy.pack_count(),
            0,
            "the harness must reproduce the pack delete"
        );

        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let packs = fx.pack_count();
        assert!(packs > 0);
        for (layer, argv) in clone_layers(&fx, &clone) {
            let ran = run_in(&argv, script, &[("MAIN", &fx.main)]);
            assert!(!ran.out.contains("PLANTED"), "{layer}: {}", ran.out);
            assert!(
                !fx.main.join(".git/objects/pack/planted").exists(),
                "{layer}"
            );
            assert_eq!(fx.pack_count(), packs, "{layer}: {}", ran.out);
            // The seat's OWN alternates are its business: it only changes what
            // the seat itself can read.
            let own = run_in(
                &argv,
                "printf '%s\\n' \"$(cat .git/objects/info/alternates)\" > \
             .git/objects/info/alternates && echo OWN",
                &[],
            );
            assert!(own.out.contains("OWN"), "{layer}: {}", own.out);
        }
    }

    /// T5 origin/HEAD and tags in main, plus T5b: the documented origin-side
    /// residual (ruling O3), pinned so a server-side guard would flip it.
    #[test]
    fn bug_2169_bwrap_t5_origin_head_and_tags() {
        if !bwrap_live() {
            return;
        }
        let script = r#"git -C "$MAIN" update-ref --no-deref refs/remotes/origin/HEAD "$EVIL" && echo HEAD_MOVED
git -C "$MAIN" tag -f v0.0.1 "$EVIL" >/dev/null 2>&1 && echo TAG_MOVED
true"#;
        let legacy = fixture();
        let settings = legacy_2694_settings(&legacy, &legacy.other_wt);
        let ran = run_in(
            &claude_sandbox_argv(&settings, &legacy.other_wt),
            &format!(
                "MAIN={} EVIL={}; {script}",
                legacy.main.display(),
                legacy.evil
            ),
            &[],
        );
        assert!(
            ran.out.contains("HEAD_MOVED") && ran.out.contains("TAG_MOVED"),
            "{}",
            ran.out
        );
        assert_eq!(legacy.rev("v0.0.1"), legacy.evil);

        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let (head, tag) = (fx.rev("refs/remotes/origin/HEAD"), fx.rev("v0.0.1"));
        for (layer, mut argv) in clone_layers(&fx, &clone) {
            let ran = run_in(
                &argv,
                &format!("MAIN={} EVIL={}; {script}", fx.main.display(), fx.evil),
                &[],
            );
            assert!(!ran.out.contains("HEAD_MOVED"), "{layer}: {}", ran.out);
            assert!(!ran.out.contains("TAG_MOVED"), "{layer}: {}", ran.out);
            assert_eq!(fx.rev("refs/remotes/origin/HEAD"), head, "{layer}");
            assert_eq!(fx.rev("v0.0.1"), tag, "{layer}");

            // T5b: a seat with a remote it can reach can push a new tag there.
            let at = argv.iter().position(|a| a == "--chdir").unwrap();
            let origin = fx.origin.display().to_string();
            argv.splice(at..at, ["--bind".to_string(), origin.clone(), origin]);
            let tag_name = format!("v9.9.9-{}", layer.replace(' ', "-"));
            let ran = run_in(
                &argv,
                &format!("git tag {tag_name} && git push -q origin {tag_name} && echo PUSHED"),
                &[],
            );
            assert!(
                ran.out.contains("PUSHED"),
                "{layer}: the origin-side tag residual is documented (O3); if this now fails, a \
             server-side guard landed and this pin should flip: {}",
                ran.out
            );
        }
    }

    /// T8: the round-1 denies on main's config and hooks hold in both layouts.
    #[test]
    fn bug_2169_bwrap_t8_main_config_and_hooks() {
        if !bwrap_live() {
            return;
        }
        let script = r#"echo '[core]' >> "$MAIN/.git/config" && echo CONFIG
touch "$MAIN/.git/hooks/pre-commit" && echo HOOK
true"#;
        let legacy = fixture();
        let settings = legacy_2694_settings(&legacy, &legacy.other_wt);
        let ran = run_in(
            &claude_sandbox_argv(&settings, &legacy.other_wt),
            script,
            &[("MAIN", &legacy.main)],
        );
        assert!(
            !ran.out.contains("CONFIG") && !ran.out.contains("HOOK"),
            "{}",
            ran.out
        );

        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        for (layer, argv) in clone_layers(&fx, &clone) {
            let ran = run_in(&argv, script, &[("MAIN", &fx.main)]);
            assert!(!ran.out.contains("CONFIG"), "{layer}: {}", ran.out);
            assert!(!ran.out.contains("HOOK"), "{layer}: {}", ran.out);
            assert!(!fx.main.join(".git/hooks/pre-commit").exists(), "{layer}");
        }
    }

    /// A clone seat can build: cargo writes the seat's own target directory and
    /// runs no compiler wrapper (ruling O2), in both layers.
    #[test]
    fn bug_2169_bwrap_clone_seat_cargo_build_uses_its_own_target_dir() {
        if !bwrap_live() {
            return;
        }
        // `CARGO` is the real cargo binary that runs this test.
        let Some(cargo) = std::env::var_os("CARGO") else {
            eprintln!("SKIP (CARGO is not set)");
            return;
        };
        let fx = fixture();
        let clone = launch_clone_seat(&fx);
        let crate_dir = clone.join("probe");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(crate_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        let target = crate::seat_clone::host_recorded_clone(&clone)
            .and_then(|c| c.cargo_target_dir)
            .expect("target dir");
        for (layer, argv) in clone_layers(&fx, &clone) {
            let ran = run_in(
                &argv,
                "test -z \"$RUSTC_WRAPPER\" && test \"${RUSTC_WRAPPER+set}\" = set && \
             cd probe && \"$CARGO_BIN\" build --offline -q && echo BUILT",
                &[("CARGO_BIN", Path::new(&cargo))],
            );
            assert!(ran.out.contains("BUILT"), "{layer}: {}", ran.out);
            assert!(target.join("debug/probe").is_file(), "{layer}");
            std::fs::remove_dir_all(target.join("debug")).unwrap();
        }
    }

    /// B3 (round 1): the interactive contained RESUME goes through os_wrap too.
    #[test]
    fn a_contained_interactive_resume_goes_through_os_wrap() {
        let fx = fixture();
        std::fs::create_dir_all(fx.other_wt.join(".aida")).unwrap();
        std::fs::write(
            fx.other_wt.join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();
        let _ambient = ambient(&fx.main);
        let _cwd = crate::session::contained_cwd_seam::pin(&fx.other_wt);
        let plain =
            crate::session::build_claude_resume_exec_command("sid-1", None, false, None).unwrap();
        assert_eq!(plain.get_program(), "claude");
        match crate::session::build_claude_resume_exec_command("sid-1", None, true, None) {
            Ok(cmd) => {
                assert_eq!(cmd.get_program(), "bwrap");
                let args: Vec<String> = cmd
                    .get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                let at = args
                    .iter()
                    .position(|a| a == "claude")
                    .expect("claude in argv");
                assert_eq!(&args[at + 1..at + 3], ["--resume", "sid-1"]);
                assert!(args
                    .windows(3)
                    .any(|w| w[0] == "--bind" && Path::new(&w[1]) == fx.other_wt));
            }
            // No usable bwrap here: the resume must refuse, never run unwrapped.
            Err(err) => assert!(format!("{err:#}").contains("Refusing"), "{err:#}"),
        }
    }

    /// B4 (round 1): a contained `--bg` launch under os_wrap is refused BEFORE
    /// it claims, leases, clones or pins anything.
    #[test]
    fn a_refused_contained_bg_launch_leaves_no_lease_clone_or_pin() {
        let fx = fixture();
        let seats = fx.root.join("seats");
        std::fs::create_dir_all(fx.main.join(".aida")).unwrap();
        std::fs::write(
            fx.main.join(".aida/config.toml"),
            format!(
                "[contained]\nseat_tree = \"clone\"\nos_wrap = true\n\n[worktree_pool]\n\
             worktree_parent = {}\n",
                aida_core::toml_quote::toml_string(&seats.display().to_string())
            ),
        )
        .unwrap();
        // The launching seat may delegate `implementer`, so without the early
        // refusal the launch would reach session_start.
        let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &fx.main,
            "advisor",
            &["implementer"],
            &[("AIDA_OS_WRAP", None)],
        );
        let config = crate::agent_launch::AgentLaunchConfig {
            agent_type: "claude",
            binary: "/usr/bin/true",
            default_args: crate::session::claude_contained_flags_for(&fx.main).unwrap(),
            prompt_style: crate::agent_launch::AgentPromptStyle::Positional,
            contained: true,
        };
        let refs_before = fx.host_refs();
        let err = crate::agent_launch::agent_new_bg_dispatch(
            config,
            Some("implementer".into()),
            Some("TASK-2169".into()),
            false,
            Some(&fx.main),
            crate::agent_launch::AgentContextOptions::new(false, false),
            false,
            false,
            false,
            false,
            crate::agent_launch::AgentPromptOptions::new(Some("work".into()), false),
            crate::agent_launch::AgentResumeOptions::new(false, None, false, false),
            crate::agent_launch::AgentDefaultFlagOptions::new(false, Vec::new(), None),
            Some("bg-refused".into()),
            None,
            false,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("background supervisor"),
            "{err:#}"
        );
        assert!(crate::list_leases(&fx.main).is_empty(), "no lease");
        assert!(!seats.exists(), "no clone directory");
        assert_eq!(fx.host_refs(), refs_before, "no pin or branch");
        assert!(!git_ok(&fx.main, &["worktree", "list"]).contains("task-2169"));
    }
}
