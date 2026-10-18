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
//! trace:BUG-1982 | ai:claude

use std::path::{Path, PathBuf};

/// Run `program` in `dir`; panics on failure, returns stdout + stderr.
fn run(dir: &Path, program: &str, args: &[&str]) -> String {
    let out = std::process::Command::new(program)
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
         && git -c user.email=p@example.com -c user.name=p commit -qm probe \
         && echo commit-ok >> '{out}' || echo commit-failed >> '{out}'\n",
        out = out.display()
    ));
    script
}

/// Run one contained `claude -p` in `tree` that executes `script`.
fn run_contained(tree: &Path, home: &Path, script: &str) -> String {
    use std::io::Write as _;
    let script_path = tree.join("probe.sh");
    std::fs::write(&script_path, script).unwrap();
    let settings = crate::session::claude_contained_settings_json(
        &crate::contained_boundary::contained_write_roots(tree),
    );
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

// Costs two model calls, so CI skips it unless `AIDA_TEST_REAL_CLAUDE=1`.
// Optional: `AIDA_TEST_CLAUDE_BIN` (default `claude`),
// `AIDA_TEST_REAL_CLAUDE_MODEL` (default `haiku`).
// trace:BUG-1982 | ai:claude
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
