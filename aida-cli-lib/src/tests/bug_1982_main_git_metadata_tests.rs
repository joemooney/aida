//! BUG-1982: a contained session cannot redirect which git directory or config
//! the host's git uses through `<main>/.git/worktrees/<wt>` metadata.
//!
//! - Real claude (env-gated, `AIDA_TEST_REAL_CLAUDE=1`):
//!   `real_claude_contained_session_cannot_redirect_main_git_metadata` runs two
//!   contained sessions with the launch settings
//!   ([`crate::session::claude_contained_settings_json`]). One runs in a linked
//!   worktree, whose metadata Claude Code's own sandbox would otherwise expose,
//!   and probes `commondir`, `gitdir`, `config.worktree` and `<main>/.git/config`.
//!   The other runs in a BUG-2169 seat clone, the contained seat layout, and
//!   commits there while the same probes fail. The fixture lives under the
//!   operator's real `~/.cache`, outside every sandbox-writable temp directory.
//!
//! - Unit (CI): `bug_1982_main_git_denied_whatever_the_store_grant` checks the
//!   launch settings deny `<main>/.git` (sandbox `denyWrite` and tool rules)
//!   both with the store grant and when the git version check withholds it.
//! - Doc (CI): `bug_1982_overview_documents_the_main_git_write_surface`.
//!
//! trace:BUG-1982 | ai:claude

use std::path::{Path, PathBuf};

/// Run `program` in `dir`; panics on failure, returns stdout + stderr.
fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let mut command = if program == "git" {
        crate::test_git::git_command()
    } else {
        std::process::Command::new(program)
    };
    let out = command
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("spawn {program}: {e}"));
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{program} {args:?} failed: {text}");
    text
}

/// A shell script that tries each metadata write, then a commit in `tree`,
/// recording each outcome in `out`.
fn probe_script(tree: &Path, metadata: &[PathBuf], out: &Path) -> String {
    let mut script = format!("cd '{}'\n", tree.display());
    for target in metadata {
        script.push_str(&format!(
            "if (printf 'gitdir: /tmp/evil\\n' >> '{t}') 2>/dev/null; then echo 'wrote {t}'; \
             else echo 'denied {t}'; fi >> '{out}'\n",
            t = target.display(),
            out = out.display()
        ));
    }
    script.push_str(&format!(
        "echo change > probe-commit.txt && git add probe-commit.txt \
         && git -c maintenance.auto=false -c gc.auto=0 -c user.email=p@example.com -c user.name=p commit -qm probe \
         && echo commit-ok >> '{out}' || echo commit-failed >> '{out}'\n",
        out = out.display()
    ));
    script
}

/// Give the fixture the same host-owned lease required of a production seat.
/// An unleased clone is deliberately denied its own .git write permission.
// trace:TASK-2053 | ai:codex
fn lease_contained_clone(main: &Path, clone: &Path) {
    aida_core::seat_clone_detect::note_host_main(main);
    let lease = crate::lease_path(main, "c0ffee001641");
    std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
    std::fs::write(
        lease,
        format!(
        "id = \"c0ffee001641\"\nscope = \"TASK-1641\"\nslug = \"task-1641\"\nowner = \"test\"\n\
         worktree_path = {}\nbranch = \"seat-1\"\nstarted_at = \"2026-10-23T00:00:00Z\"\n\
         hostname = \"test\"\ntree_kind = \"clone\"\n",
        aida_core::toml_quote::toml_string(&clone.display().to_string()),
    ),
    )
    .unwrap();
    assert!(matches!(
        crate::seat_clone::main_for(clone),
        crate::seat_clone::MainFor::Verified(_)
    ));
    assert!(
        crate::seat_clone::host_recorded_clone(clone).is_some(),
        "fixture must model a leased seat"
    );
}

/// Run one contained `claude -p` in `tree` that executes `script`.
fn run_contained(tree: &Path, home: &Path, script: &str) -> String {
    use std::io::Write as _;
    let script_path = tree.join("probe.sh");
    std::fs::write(&script_path, script).unwrap();
    let settings = crate::session::claude_contained_settings_json(
        &crate::contained_boundary::contained_write_roots(tree),
    )
    .unwrap();
    let prompt = format!(
        "Run exactly this one Bash command and nothing else: sh {}",
        script_path.display()
    );
    let program = std::env::var("AIDA_TEST_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string());
    let model =
        std::env::var("AIDA_TEST_REAL_CLAUDE_MODEL").unwrap_or_else(|_| "haiku".to_string());
    let mut child = std::process::Command::new(program)
        .args([
            "-p",
            "--model",
            model.as_str(),
            "--permission-mode",
            "dontAsk",
            "--setting-sources",
            "project",
            "--settings",
            settings.as_str(),
            "--output-format",
            "json",
            "--allowed-tools",
            "Bash",
        ])
        .current_dir(tree)
        .env("HOME", home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn claude");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let transcript = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "claude failed: {transcript}");
    transcript
}

/// `sandbox.filesystem.denyWrite` and `permissions.deny` of `settings`.
fn denies(settings: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    (
        strings(&settings["sandbox"]["filesystem"]["denyWrite"]),
        strings(&settings["permissions"]["deny"]),
    )
}

// The launch settings for a contained session in a linked worktree deny the
// whole main git directory in both layers, which covers
// `worktrees/<wt>/{commondir,gitdir,config.worktree}`. That holds when the
// store grant is withheld too (git older than 2.40, or an unparsable
// version): the deny never depends on the grant.
// trace:BUG-1982.ace25510 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1982_main_git_denied_whatever_the_store_grant() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let main = parent.join("main");
    std::fs::create_dir_all(&main).unwrap();
    run(&main, "git", &["init", "-q", "-b", "main"]);
    run(
        &main,
        "git",
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let worktree = parent.join("wt");
    run(
        &main,
        "git",
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            worktree.to_str().unwrap(),
        ],
    );
    let main_git = main.join(".git");
    let sandbox_entry = format!("/{}", main_git.display());
    let tool_rule = format!("Edit(/{}/**)", main_git.display());
    let roots = crate::contained_boundary::contained_write_roots(&worktree);
    for version in ["git version 2.43.0", "git version 2.39.5", "not a version"] {
        aida_core::git_ops::with_host_git_version(version, || {
            let json = crate::session::claude_contained_settings_json(&roots).unwrap();
            let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
            let (deny_write, deny) = denies(&settings);
            assert!(
                deny_write.contains(&sandbox_entry),
                "{version}: sandbox denyWrite must cover {}: {settings}",
                main_git.display()
            );
            assert!(
                deny.contains(&tool_rule),
                "{version}: tool rules must deny {}: {settings}",
                main_git.display()
            );
            let granted = settings["sandbox"]["filesystem"]["allowWrite"]
                .as_array()
                .is_some_and(|a| {
                    a.iter().any(|entry| {
                        entry.as_str()
                            == Some(format!("/{}", main.join(".aida-store").display()).as_str())
                    })
                });
            assert_eq!(
                granted,
                version == "git version 2.43.0",
                "{version}: store grant only on a supported git: {settings}"
            );
            // The separate mailbox grant remains narrow on every Git version.
            // trace:BUG-2218 | ai:codex
            let entries = |key: &str| {
                settings["sandbox"]["filesystem"][key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            };
            let allows = entries("allowWrite");
            let denies = entries("denyWrite");
            let covers = |entries: &[String], target: &Path| {
                entries.iter().any(|entry| {
                    entry
                        .strip_prefix('/')
                        .is_some_and(|path| target.starts_with(path))
                })
            };
            let writable = |path: &Path| covers(&allows, path) && !covers(&denies, path);
            assert!(writable(&main.join(".aida/mailbox/message.json")));
            for path in [
                main_git.join("config"),
                main_git.join("worktrees/wt/commondir"),
                main_git.join("worktrees/wt/gitdir"),
                main_git.join("worktrees/wt/config.worktree"),
            ] {
                assert!(
                    !writable(&path),
                    "{version}: metadata writable: {}",
                    path.display()
                );
                assert!(!crate::contained_boundary::evaluator::allowed(
                    &settings, "Edit", &path, &worktree, &parent
                ));
            }
        });
    }
    // Every redirect target lies under the denied directory.
    let meta = main_git.join("worktrees").join("wt");
    for name in ["commondir", "gitdir", "config.worktree"] {
        assert!(meta.join(name).starts_with(&main_git));
    }
}

// A main checkout whose path holds a rule metacharacter cannot be denied
// exactly, so the contained launch is refused with an error naming the path,
// and no settings are built.
// trace:BUG-1982 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1982_unexpressible_main_git_deny_refuses_the_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let main = parent.join("ma[in]");
    std::fs::create_dir_all(&main).unwrap();
    run(&main, "git", &["init", "-q", "-b", "main"]);
    run(
        &main,
        "git",
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let worktree = parent.join("wt");
    run(
        &main,
        "git",
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            worktree.to_str().unwrap(),
        ],
    );
    let roots = crate::contained_boundary::contained_write_roots(&worktree);
    assert!(
        !crate::contained_boundary::write_allow_rules(&roots).is_empty(),
        "the worktree itself is expressible, so only the missing deny refuses the launch"
    );
    // Review round 1 F1: Claude Code's own sandbox leaves `<main>/.git`
    // writable to a linked-worktree session, so withholding grants is not
    // enough. The launch is refused and no settings (no flags) are produced.
    aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
        let err = crate::session::claude_contained_settings_json(&roots)
            .expect_err("an unexpressible main git deny must refuse the launch")
            .to_string();
        assert!(
            err.contains("refusing to launch a contained session"),
            "{err}"
        );
        assert!(
            err.contains(&main.join(".git").display().to_string()),
            "{err}"
        );
        assert!(
            crate::session::claude_contained_flags_for(&worktree).is_err(),
            "no contained launch flags are built"
        );
    });

    // Second line of defence in the pure builder: no write grants.
    let json = crate::session::contained_settings_json_with_boundary(
        &[],
        false,
        &roots,
        &[],
        &[main.join(".git")],
    );
    let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        settings["permissions"]["allow"]
            .as_array()
            .is_none_or(|a| a.is_empty()),
        "{settings}"
    );

    // Mutation control: the same launch with an expressible main path grants.
    let json = crate::session::contained_settings_json_with_boundary(
        &[],
        false,
        &roots,
        &[],
        &[parent.join("plain").join(".git")],
    );
    let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(
        settings["permissions"]["allow"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{settings}"
    );
}

// Review round 1 F3: a checkout reached through a symlink has two spellings
// of its git directory, and a tool or sandbox path may resolve to either, so
// the launch denies both (the path as given and the canonical one).
// trace:BUG-1982 | ai:claude
#[cfg(unix)]
#[test]
fn bug_1982_symlinked_checkout_denies_every_main_git_spelling() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let real = parent.join("real-main");
    std::fs::create_dir_all(&real).unwrap();
    run(&real, "git", &["init", "-q", "-b", "main"]);
    run(
        &real,
        "git",
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let link = parent.join("linked-main");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let paths = crate::session::main_git_deny_paths(&link);
    assert!(paths.contains(&link.join(".git")), "{paths:?}");
    assert!(paths.contains(&real.join(".git")), "{paths:?}");

    // End to end: a session whose code root is the symlinked checkout.
    let roots = crate::contained_boundary::contained_write_roots(&link);
    aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
        let json = crate::session::claude_contained_settings_json(&roots).unwrap();
        let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
        let (deny_write, deny) = denies(&settings);
        for spelling in [link.join(".git"), real.join(".git")] {
            let entry = format!("/{}", spelling.display());
            assert!(deny_write.contains(&entry), "{entry}: {settings}");
            let rule = format!("Edit(/{}/**)", spelling.display());
            assert!(deny.contains(&rule), "{rule}: {settings}");
        }
    });
}

// The remaining `<main>/.git` write surface, its condition and the
// linked-worktree commit limit are documented.
// trace:BUG-1982.ac549f55 | ai:claude
#[test]
fn bug_1982_overview_documents_the_main_git_write_surface() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let overview = std::fs::read_to_string(workspace.join("OVERVIEW.md")).unwrap();
    let start = overview
        .find("A contained session has no write access to the main git directory")
        .expect("OVERVIEW.md documents the main git directory boundary");
    let section = &overview[start..];
    let section = &section[..section.find("trace:BUG-1982").expect("traced section")];
    for needle in [
        "commondir",
        "gitdir",
        "config.worktree",
        "`.git` pointer",
        "While these denies are in place",
        "cannot commit",
        "BUG-2169",
        "launch is refused",
        "symlinked checkout",
    ] {
        assert!(section.contains(needle), "missing {needle:?}: {section}");
    }
}

// Costs two model calls, so CI skips it unless `AIDA_TEST_REAL_CLAUDE=1`.
// Optional: `AIDA_TEST_CLAUDE_BIN` (default `claude`),
// `AIDA_TEST_REAL_CLAUDE_MODEL` (default `haiku`).
// trace:BUG-1982.ac7f9407 | ai:claude
// trace:BUG-1982.ace25510 | ai:claude
#[cfg(unix)]
#[test]
fn real_claude_contained_session_cannot_redirect_main_git_metadata() {
    if std::env::var("AIDA_TEST_REAL_CLAUDE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set AIDA_TEST_REAL_CLAUDE=1 to drive a real `claude -p`");
        return;
    }
    // The operator's real home on purpose: inside Claude Code `TMPDIR` is
    // sandbox-writable, so a temp-dir fixture would pass with or without the
    // boundary.
    let home = crate::test_home::passwd_home().expect("password-database home");
    let cache_home = home.join(".cache");
    std::fs::create_dir_all(&cache_home).unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("aida-test-bug1982-")
        .tempdir_in(&cache_home)
        .unwrap();
    let parent = std::fs::canonicalize(fixture.path()).unwrap();
    let sandbox_tmp = std::env::temp_dir();
    let sandbox_tmp = std::fs::canonicalize(&sandbox_tmp).unwrap_or(sandbox_tmp);
    assert!(
        !parent.starts_with(&sandbox_tmp),
        "the fixture must sit outside the sandbox-writable {}: {}",
        sandbox_tmp.display(),
        parent.display()
    );

    let origin = parent.join("origin.git");
    run(
        &parent,
        "git",
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    let main = parent.join("main");
    std::fs::create_dir_all(&main).unwrap();
    run(&main, "git", &["init", "-q", "-b", "main"]);
    run(&main, "git", &["config", "user.email", "test@example.com"]);
    run(&main, "git", &["config", "user.name", "Test"]);
    std::fs::write(main.join("README.md"), "fixture\n").unwrap();
    run(&main, "git", &["add", "README.md"]);
    run(&main, "git", &["commit", "-q", "-m", "init"]);
    run(
        &main,
        "git",
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    run(&main, "git", &["push", "-q", "origin", "main"]);
    run(
        &main,
        "git",
        &["worktree", "add", "-q", "--detach", ".aida-store"],
    );

    let worktree = parent.join("aida-pool-fixture-0");
    run(
        &main,
        "git",
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pool-0",
            worktree.to_str().unwrap(),
        ],
    );
    let meta = main.join(".git/worktrees/aida-pool-fixture-0");
    let metadata = vec![
        meta.join("commondir"),
        meta.join("gitdir"),
        meta.join("config.worktree"),
        main.join(".git/config"),
    ];
    let read = |p: &Path| std::fs::read(p).ok();
    let before: Vec<Option<Vec<u8>>> = metadata.iter().map(|p| read(p)).collect();
    // Claude Code's sandbox denies a missing path by leaving an empty file
    // there; empty carries no config, so it counts as unchanged.
    let assert_unchanged = |label: &str, probe: &str| {
        for (path, before) in metadata.iter().zip(&before) {
            let after = read(path).filter(|b| before.is_some() || !b.is_empty());
            assert_eq!(
                &after,
                before,
                "{label}: {} must not be writable from a contained session: {probe}",
                path.display()
            );
        }
    };

    // A session in the linked worktree: the redirect targets stay read-only.
    let probe_out = worktree.join("probe.out");
    run_contained(
        &worktree,
        &home,
        &probe_script(&worktree, &metadata, &probe_out),
    );
    let probe = std::fs::read_to_string(&probe_out)
        .unwrap_or_else(|e| panic!("the linked-worktree probe never ran: {e}"));
    assert!(!probe.contains("wrote "), "{probe}");
    assert_unchanged("linked worktree", &probe);
    // A linked worktree's index and objects live under the denied
    // `<main>/.git`, so a contained session there cannot commit; the seat
    // clone below is the contained commit path (BUG-2169).
    assert!(
        probe.lines().any(|l| l.trim() == "commit-failed"),
        "a contained session in a linked worktree cannot commit: {probe}"
    );
    let pool_head = run(&worktree, "git", &["rev-parse", "HEAD"]);
    assert_eq!(
        pool_head,
        run(&main, "git", &["rev-parse", "main"]),
        "the linked worktree's branch must not move: {probe}"
    );
    // The host still resolves the worktree to main's repository.
    let common = run(
        &worktree,
        "git",
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    assert_eq!(
        std::fs::canonicalize(common.trim()).unwrap(),
        std::fs::canonicalize(main.join(".git")).unwrap()
    );

    // A session in its seat clone (BUG-2169): the same probes fail, and an
    // ordinary commit in the session's own tree succeeds.
    let clone = parent.join(format!(
        "{}fixture-1",
        aida_core::seat_clone_detect::CLONE_DIR_PREFIX
    ));
    crate::seat_clone::create(&main, &clone, "seat-1", "main").expect("create the seat clone");
    lease_contained_clone(&main, &clone);
    let head_before = run(&clone, "git", &["rev-parse", "HEAD"]);
    let clone_out = clone.join("probe.out");
    run_contained(&clone, &home, &probe_script(&clone, &metadata, &clone_out));
    let probe = std::fs::read_to_string(&clone_out)
        .unwrap_or_else(|e| panic!("the seat-clone probe never ran: {e}"));
    assert!(!probe.contains("wrote "), "{probe}");
    assert!(
        probe.lines().any(|l| l.trim() == "commit-ok"),
        "a contained seat must commit in its own tree: {probe}"
    );
    assert_ne!(run(&clone, "git", &["rev-parse", "HEAD"]), head_before);
    assert_unchanged("seat clone", &probe);
}

// trace:TASK-2053 | ai:codex
#[test]
fn task_1641_fixture_requires_a_lease_to_keep_clone_git_writable() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    std::fs::create_dir(&main).unwrap();
    run(&main, "git", &["init", "-q", "-b", "main"]);
    run(
        &main,
        "git",
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "init",
        ],
    );
    let origin = tmp.path().join("origin.git");
    run(
        tmp.path(),
        "git",
        &[
            "clone",
            "--bare",
            main.to_str().unwrap(),
            origin.to_str().unwrap(),
        ],
    );
    run(
        &main,
        "git",
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let clone = tmp.path().join("aida-seat-fixture-1");
    crate::seat_clone::create(&main, &clone, "seat-1", "main").unwrap();
    let roots = crate::contained_boundary::contained_write_roots(&clone);
    assert!(
        crate::contained_boundary::contained_main_checkout_git_deny_paths(&roots)
            .contains(&clone.join(".git"))
    );
    lease_contained_clone(&main, &clone);
    assert!(
        !crate::contained_boundary::contained_main_checkout_git_deny_paths(&roots)
            .contains(&clone.join(".git"))
    );
    let settings: serde_json::Value =
        serde_json::from_str(&crate::session::claude_contained_settings_json(&roots).unwrap())
            .unwrap();
    let (write_denies, _) = denies(&settings);
    assert!(
        write_denies.contains(&format!("/{}", main.join(".git").display())),
        "{settings}"
    );
    assert!(
        !write_denies.contains(&format!("/{}", clone.join(".git").display())),
        "{settings}"
    );
    assert!(
        settings["sandbox"]["filesystem"]["denyRead"].is_null(),
        "{settings}"
    );
}
