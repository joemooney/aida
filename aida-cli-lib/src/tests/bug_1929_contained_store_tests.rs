//! BUG-1929: the main checkout's store and cache in the contained write set.
//!
//! Which test is which:
//! - Unit: the generated `sandbox.filesystem.allowWrite` is exactly the store
//!   directory and the cache files, nothing else in the main checkout, and
//!   `denyWrite` protects the store pointer and host-owned git authority.
//! - Harness (always runs): a small model of how Claude Code's Linux sandbox
//!   applies `allowWrite` and `denyWrite` (each existing entry bound as is, no
//!   glob expansion, deny over allow), checked over real tempdir paths.
//! - Real claude (env-gated, `AIDA_TEST_REAL_CLAUDE=1`):
//!   `real_claude_contained_session_claims_and_comments` runs `aida` inside a
//!   contained session in a linked worktree, probes writes inside and outside
//!   the write set, plants an embedded repository in the store (E1) and
//!   checks the host's store sync refuses it without running its
//!   `core.fsmonitor`, plants a second one whose `uploadpack` a recursing
//!   fetch would run (F1) and checks `aida fetch --store-only` does not, and
//!   reads the result from main. Its fixture lives under
//!   the operator's real `~/.cache`, outside every sandbox-writable temp
//!   directory.
//! - Guard: `bug_1929_store_git_guard` fails on any new raw
//!   `Command::new("git")` aimed at the store; build host store git through
//!   `aida_core::git_ops::host_git_command`.
//! - F1 regression: `bug_1929_f1_store_fetch_never_runs_a_planted_uploadpack`
//!   moves a gitlink on origin over a planted embedded repository and checks
//!   `fetch_branch`, `refresh_remote_branch_state` and the push refusal.
//!
//! trace:BUG-1929 | ai:claude

use super::*;
use std::path::{Path, PathBuf};

const CACHE_FILES: [&str; 5] = [
    "cache.db",
    "cache.db-wal",
    "cache.db-shm",
    "cache.db.refresh.lock",
    "cache.db.refresh-request.lock",
];

/// `<tmp>/main` holding an attached-store-shaped `.aida-store` and a cache
/// with its sidecars, plus the rest of a checkout the session must not write.
fn main_fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let main = std::fs::canonicalize(tmp.path()).unwrap().join("main");
    std::fs::create_dir_all(main.join(".aida-store/objects")).unwrap();
    std::fs::create_dir_all(main.join(".aida-store/.aida")).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::create_dir_all(main.join(".git")).unwrap();
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::write(
        main.join(".aida-store/.git"),
        format!(
            "gitdir: {}\n",
            main.join(".git/worktrees/-aida-store").display()
        ),
    )
    .unwrap();
    for name in CACHE_FILES {
        std::fs::write(main.join(".aida").join(name), b"").unwrap();
    }
    std::fs::write(main.join(".aida/config.toml"), b"").unwrap();
    (tmp, main)
}

fn settings_with_store(worktree: &Path, main: &Path) -> serde_json::Value {
    let json = crate::session::contained_settings_json_with_store(
        &[],
        false,
        &contained_write_roots(worktree),
        &contained_store_write_paths(main),
    );
    serde_json::from_str(&json).unwrap()
}

fn allow_write(settings: &serde_json::Value) -> Vec<String> {
    filesystem_entries(settings, "allowWrite")
}

fn deny_write(settings: &serde_json::Value) -> Vec<String> {
    filesystem_entries(settings, "denyWrite")
}

fn filesystem_entries(settings: &serde_json::Value, key: &str) -> Vec<String> {
    settings
        .pointer(&format!("/sandbox/filesystem/{key}"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// How Claude Code's Linux sandbox applies `allowWrite` and `denyWrite`
/// (claude 2.1.296): a `//p` entry is the absolute path `/p`; it is bound only
/// when it exists at launch, with no glob expansion; a directory bind covers
/// everything under it; a `denyWrite` bind is read-only and wins over an
/// enclosing `allowWrite`.
fn sandbox_may_write(settings: &serde_json::Value, target: &Path) -> bool {
    bound_covers(&allow_write(settings), target) && !bound_covers(&deny_write(settings), target)
}

fn bound_covers(entries: &[String], target: &Path) -> bool {
    entries.iter().any(|entry| {
        let Some(abs) = entry.strip_prefix('/') else {
            return false;
        };
        let bound = Path::new(abs);
        if !bound.exists() {
            return false;
        }
        if bound.is_dir() {
            target.starts_with(bound)
        } else {
            target == bound
        }
    })
}

// UNIT: the entries are exactly the store directory and the cache files.
#[test]
fn settings_allow_write_is_exactly_store_and_cache() {
    let (_tmp, main) = main_fixture();
    let worktree = main.parent().unwrap().join("aida-pool-main-0");
    std::fs::create_dir_all(&worktree).unwrap();
    let settings = settings_with_store(&worktree, &main);

    let mut expected = vec![format!("/{}", main.join(".aida-store").display())];
    for name in CACHE_FILES {
        expected.push(format!("/{}", main.join(".aida").join(name).display()));
    }
    assert_eq!(allow_write(&settings), expected);
    assert_eq!(
        deny_write(&settings),
        vec![
            format!("/{}", main.join(".aida-store/.git").display()),
            format!("/{}", main.join(".git").display())
        ],
        "the store's pointer and host-owned git directory are denied"
    );
    for entry in allow_write(&settings) {
        assert!(entry.starts_with("//"), "absolute anchor: {entry}");
        assert!(!entry.contains('*'), "no globs (Linux skips them): {entry}");
    }

    // The Edit/Write tool boundary is still the worktree alone.
    let allow = settings["permissions"]["allow"].as_array().unwrap();
    assert!(!allow.is_empty());
    for rule in allow {
        let rule = rule.as_str().unwrap();
        assert!(
            rule.contains(&worktree.display().to_string()),
            "tool rule outside the worktree: {rule}"
        );
    }
}

// BEHAVIOURAL (harness): over real paths, the store and cache are writable and
// the rest of the main checkout is not.
#[test]
fn sandbox_boundary_harness_allows_store_and_cache_denies_rest_of_main() {
    let (_tmp, main) = main_fixture();
    let worktree = main.parent().unwrap().join("aida-pool-main-0");
    std::fs::create_dir_all(&worktree).unwrap();
    let settings = settings_with_store(&worktree, &main);

    for allowed in [
        main.join(".aida-store/objects/BUG-1.yaml"),
        main.join(".aida-store/.aida/store-write.lock"),
        main.join(".aida/cache.db"),
        main.join(".aida/cache.db-wal"),
        main.join(".aida/cache.db-shm"),
    ] {
        assert!(
            sandbox_may_write(&settings, &allowed),
            "{}",
            allowed.display()
        );
    }
    for denied in [
        main.join(".aida-store/.git"),
        main.join(".aida/config.toml"),
        main.join(".aida/cache.db.bak"),
        main.join(".aida/agents.toml"),
        main.join(".git/config"),
        main.join("src/lib.rs"),
        main.clone(),
    ] {
        assert!(
            !sandbox_may_write(&settings, &denied),
            "{}",
            denied.display()
        );
    }
}

// A symlinked main checkout yields both spellings, each still only the store
// and cache.
#[cfg(unix)]
#[test]
fn symlinked_main_lists_both_spellings_of_the_same_files() {
    let (_tmp, main) = main_fixture();
    let link = main.parent().unwrap().join("main-link");
    std::os::unix::fs::symlink(&main, &link).unwrap();
    let paths = contained_store_write_paths(&link);
    assert_eq!(paths.len(), 2 * (1 + CACHE_FILES.len()));
    for path in &paths {
        let base_ok = path.starts_with(link.join(".aida-store"))
            || path.starts_with(main.join(".aida-store"))
            || path.parent() == Some(link.join(".aida").as_path())
            || path.parent() == Some(main.join(".aida").as_path());
        assert!(base_ok, "{}", path.display());
    }
    let denied = contained_store_deny_write_paths(&paths);
    assert_eq!(
        denied,
        vec![
            link.join(".aida-store/.git"),
            link.join(".git"),
            main.join(".aida-store/.git"),
            main.join(".git")
        ],
        "each spelling of the store gets its .git pointer denied"
    );
    let settings = crate::session::contained_settings_json_with_store(&[], false, &[], &paths);
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(deny_write(&settings).len(), 4, "{settings}");
}

// Fail closed: the store directory is never allowed without its `.git` deny,
// and the cache files alone emit no deny entry.
#[test]
fn store_allow_always_carries_its_git_deny() {
    let (_tmp, main) = main_fixture();
    let paths = contained_store_write_paths(&main);
    let fs = sandbox_filesystem(&paths).expect("expressible main emits a filesystem key");
    let allow: Vec<&str> = fs["allowWrite"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let deny: Vec<&str> = fs["denyWrite"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for store in allow.iter().filter(|e| e.ends_with("/.aida-store")) {
        assert!(deny.contains(&format!("{store}/.git").as_str()), "{fs}");
    }

    let cache_only: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.file_name().is_none_or(|n| n != ".aida-store"))
        .collect();
    let fs = sandbox_filesystem(&cache_only).unwrap();
    assert_eq!(fs["denyWrite"], serde_json::json!([]), "{fs}");
}

// A main checkout path that cannot be expressed exactly adds no entry, and the
// settings keep the pre-BUG-1929 shape (no `filesystem` key).
#[test]
fn unexpressible_main_emits_no_entries() {
    for main in ["/work/a*b", "relative/main"] {
        let main = Path::new(main);
        assert!(sandbox_allow_write_entries(&contained_store_write_paths(main)).is_empty());
        let json = crate::session::contained_settings_json_with_store(
            &[],
            false,
            &[],
            &contained_store_write_paths(main),
        );
        let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(settings.pointer("/sandbox/filesystem").is_none(), "{json}");
    }
    let json = crate::session::contained_settings_json(&[], false, &[]);
    let settings: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(settings.pointer("/sandbox/filesystem").is_none(), "{json}");
}

/// The settings JSON a contained launch gets for `write_roots`, parsed.
// trace:BUG-1929 | ai:claude
fn launch_settings(write_roots: &[PathBuf]) -> (String, serde_json::Value) {
    let flags = crate::session::claude_contained_flags_for_roots(write_roots);
    let json = flags[3].clone();
    let settings = serde_json::from_str(&json).unwrap();
    (json, settings)
}

// FAIL CLOSED: with no write roots (a session outside git, or a resume whose
// cwd is not a git root) the launch grants no store or cache path at all. It
// never falls back to the main checkout of the process's cwd (under
// `cargo test`, this repository's own checkout).
// trace:BUG-1929 | ai:claude
#[test]
fn bug_1929_outside_git_grants_no_store_or_cache_paths() {
    let (json, settings) = launch_settings(&[]);
    assert!(allow_write(&settings).is_empty(), "{json}");
    assert!(deny_write(&settings).is_empty(), "{json}");
    assert!(!json.contains(".aida-store"), "{json}");
    assert!(!json.contains("cache.db"), "{json}");
}

// A linked worktree's launch grants the store and cache of ITS OWN main
// checkout, and nothing belonging to any other checkout (in particular not
// the one the process's cwd is in).
// trace:BUG-1929 | ai:claude
#[test]
fn bug_1929_linked_worktree_grants_its_own_main_only() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let main = base.join("main");
    std::fs::create_dir_all(&main).unwrap();
    bug_1929_git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("README.md"), "hi\n").unwrap();
    bug_1929_git(&main, &["add", "README.md"]);
    bug_1929_git(&main, &["commit", "-qm", "init"]);
    std::fs::create_dir_all(main.join(".aida-store")).unwrap();
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    let worktree = base.join("wt");
    bug_1929_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "wt",
            worktree.to_str().unwrap(),
        ],
    );

    let (json, settings) = launch_settings(&[worktree.clone()]);
    let mut expected = vec![format!("/{}", main.join(".aida-store").display())];
    for name in CACHE_FILES {
        expected.push(format!("/{}", main.join(".aida").join(name).display()));
    }
    assert_eq!(allow_write(&settings), expected, "{json}");
    assert_eq!(
        deny_write(&settings),
        vec![
            format!("/{}", main.join(".aida-store/.git").display()),
            format!("/{}", main.join(".git").display())
        ],
        "{json}"
    );
    assert_eq!(
        json.matches(".aida-store").count(),
        2 + 2 * WRITE_TOOLS.len(),
        "only this main's store and the sandbox/tool denies for its .git pointer: {json}"
    );
}

// DOCTOR: no gap when everything is in place; one per missing sidecar, store
// elsewhere, or cache elsewhere.
#[test]
fn store_boundary_gaps_name_each_exclusion() {
    let (_tmp, main) = main_fixture();
    let store = main.join(".aida-store");
    let cache = main.join(".aida/cache.db");
    assert!(contained_store_boundary_gaps(&main, Some(&store), &cache).is_empty());

    std::fs::remove_file(main.join(".aida/cache.db-wal")).unwrap();
    let gaps = contained_store_boundary_gaps(&main, Some(&store), &cache);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].contains("cache.db-wal"), "{gaps:?}");
    std::fs::write(main.join(".aida/cache.db-wal"), b"").unwrap();

    let elsewhere = main.parent().unwrap().join("other-store");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let gaps = contained_store_boundary_gaps(&main, Some(&elsewhere), &cache);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].contains("AIDA store"), "{gaps:?}");

    let other_cache = main.parent().unwrap().join("cache.db");
    std::fs::write(&other_cache, b"").unwrap();
    let gaps = contained_store_boundary_gaps(&main, Some(&store), &other_cache);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].contains("AIDA cache"), "{gaps:?}");

    let gaps = contained_store_boundary_gaps(Path::new("/work/a*b"), None, &cache);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert!(gaps[0].contains("cannot be expressed"), "{gaps:?}");
}

// REAL CLAUDE (env-gated): in a linked worktree of a fresh AIDA project, a
// contained `claude -p` session runs one probe script through sandboxed Bash.
// It claims a spec (`aida edit --status in-progress`), comments on it, finishes
// a queued spec (`aida queue done`), then tries to write inside and outside
// the write set. From the host the test then checks that the claim, comment
// and completion are visible from main and committed to the store, that
// `<main>/.aida-store/objects` was writable, and that `<main>/.aida-store/.git`,
// `<main>/.aida/config.toml` and `<main>/README.md` are unchanged.
//
// The fixture lives under the real `~/.cache`, not `tempfile::tempdir()`: inside
// Claude Code `TMPDIR` is sandbox-writable, so a temp-dir fixture would pass
// with or without the write set. Costs a model call, so CI skips it unless
// `AIDA_TEST_REAL_CLAUDE=1`. Optional: `AIDA_TEST_CLAUDE_BIN` (default
// `claude`), `AIDA_TEST_AIDA_BIN` (default `aida`),
// `AIDA_TEST_REAL_CLAUDE_MODEL` (default `haiku`).
#[test]
fn real_claude_contained_session_claims_and_comments() {
    if std::env::var("AIDA_TEST_REAL_CLAUDE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set AIDA_TEST_REAL_CLAUDE=1 to drive a real `claude -p`");
        return;
    }
    use aida_core::db::DatabaseBackend as _;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let aida = std::env::var("AIDA_TEST_AIDA_BIN").unwrap_or_else(|_| "aida".to_string());
    let run = |dir: &Path, program: &str, args: &[&str]| -> String {
        let out = Command::new(program)
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
    };

    // The operator's real home on purpose: the lib-test harness points `$HOME`
    // at a temp home under `TMPDIR`, the sandbox-writable place this fixture
    // must avoid. `claude` gets this home too, for its credentials; contained
    // writes there stay outside the write set.
    let home = crate::test_home::passwd_home().expect("password-database home");
    let cache_home = home.join(".cache");
    std::fs::create_dir_all(&cache_home).unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("aida-test-bug1929-")
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
    let main = parent.join("main");
    std::fs::create_dir_all(&main).unwrap();
    run(&main, "git", &["init", "-q", "-b", "main"]);
    run(&main, "git", &["config", "user.email", "test@example.com"]);
    run(&main, "git", &["config", "user.name", "Test"]);
    std::fs::write(main.join("README.md"), "fixture\n").unwrap();
    run(&main, "git", &["add", "README.md"]);
    run(&main, "git", &["commit", "-q", "-m", "init"]);
    run(&main, &aida, &["init", "--no-skills", "--no-hooks"]);

    // Seed two approved specs and queue the second, through the store API: the
    // CLI would need advisor and dispatch authority this fixture has no grant
    // for.
    let store = main.join(".aida-store");
    let cache = main.join(".aida/cache.db");
    {
        let backend = aida_core::CachedGitBackend::open(&store, &cache).unwrap();
        let mut queued = None;
        for spec in ["BUG-1", "BUG-2"] {
            let mut req =
                aida_core::Requirement::new(format!("Contained fixture {spec}"), "d".into());
            req.spec_id = Some(spec.to_string());
            req.status = aida_core::RequirementStatus::Approved;
            queued = Some(backend.add_requirement(req).unwrap().id);
        }
        backend
            .queue_add(aida_core::QueueEntry {
                user_id: "fixture".into(),
                requirement_id: queued.unwrap(),
                position: 0,
                added_by: "fixture".into(),
                note: None,
                added_at: chrono::Utc::now(),
                for_role: None,
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .unwrap();
    }

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
    aida_core::db::prime_cache_sidecars(&cache);
    let store_head_before = run(&store, "git", &["rev-parse", "HEAD"]);
    let read = |p: &Path| std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let unchanged = [
        store.join(".git"),
        main.join(".aida/config.toml"),
        main.join("README.md"),
    ];
    let before: Vec<Vec<u8>> = unchanged.iter().map(|p| read(p)).collect();
    let store_probe = store.join("objects/x.yaml");

    // One script, so the model has a single command to run. Each write probe
    // records its outcome; the host re-checks every file itself.
    let probe_out = worktree.join("probe.out");
    let mut script = format!(
        "{aida} edit BUG-1 --status in-progress && {aida} comment add BUG-1 contained-ok \
         && {aida} queue done BUG-2 --user fixture -y\n\
         echo \"aida=$?\" > {out}\n",
        out = probe_out.display()
    );
    for target in unchanged.iter().chain(std::iter::once(&store_probe)) {
        script.push_str(&format!(
            "if (echo probe >> '{t}') 2>/dev/null; then echo 'wrote {t}'; \
             else echo 'denied {t}'; fi >> {out}\n",
            t = target.display(),
            out = probe_out.display()
        ));
    }
    // E1: plant an embedded repository in the store the way a contained agent
    // could, its own config naming a hook that appends to a marker outside the
    // write set. Inside the sandbox the marker cannot be written; it appears
    // only if a host-side git run over the store executes the hook.
    // trace:BUG-1929 | ai:claude
    let marker = parent.join("fsmonitor-ran");
    let evil_hook = store.join(".evil-fsm.sh");
    let evil_sub = store.join("objects/zz");
    script.push_str(&format!(
        "if git init -q --separate-git-dir='{gitdir}' '{sub}' \
         && printf '#!/bin/sh\\necho ran >> {marker}\\nexit 1\\n' > '{hook}' \
         && chmod 755 '{hook}' && echo x > '{sub}/x.txt' && git -C '{sub}' add x.txt \
         && git -C '{sub}' -c user.email=p@example.com -c user.name=p commit -qm planted \
         && git -C '{sub}' config core.fsmonitor '{hook}'; \
         then echo planted; else echo plant-failed; fi >> {out} 2>&1\n\
         if git -C '{store}' add objects/zz >/dev/null 2>&1; then echo staged; \
         else echo not-staged; fi >> {out}\n",
        gitdir = store.join(".evil.git").display(),
        sub = evil_sub.display(),
        hook = evil_hook.display(),
        marker = marker.display(),
        store = store.display(),
        out = probe_out.display()
    ));
    // F1: plant a second embedded repository outside `objects`, whose config
    // names an `uploadpack` that appends to a marker outside the write set. A
    // host `git fetch` that recurses into it on demand runs that command.
    // trace:BUG-1929 | ai:claude
    let up_marker = parent.join("uploadpack-ran");
    let up_hook = store.join(".evil-up.sh");
    let f1_sub = store.join("f1-sub");
    let f1_origin = parent.join("f1-origin.git");
    script.push_str(&format!(
        "if printf '#!/bin/sh\\necho ran >> {marker}\\nexit 1\\n' > '{hook}' \
         && chmod 755 '{hook}' && git init -q '{sub}' \
         && git -C '{sub}' config remote.origin.url '{origin}' \
         && git -C '{sub}' config remote.origin.uploadpack '{hook}' \
         && git -C '{sub}' config protocol.file.allow always; \
         then echo f1-planted; else echo f1-plant-failed; fi >> {out} 2>&1\n",
        hook = up_hook.display(),
        sub = f1_sub.display(),
        origin = f1_origin.display(),
        marker = up_marker.display(),
        out = probe_out.display()
    ));
    let script_path = worktree.join("probe.sh");
    std::fs::write(&script_path, script).unwrap();

    let settings = crate::session::contained_settings_json_with_store(
        &[],
        false,
        &contained_write_roots(&worktree),
        &contained_store_write_paths(&main),
    );
    let prompt = format!(
        "Run exactly this one Bash command and nothing else: sh {}",
        script_path.display()
    );
    let program = std::env::var("AIDA_TEST_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string());
    let model =
        std::env::var("AIDA_TEST_REAL_CLAUDE_MODEL").unwrap_or_else(|_| "haiku".to_string());
    let mut child = Command::new(program)
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
        .current_dir(&worktree)
        .env("HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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

    let probe = std::fs::read_to_string(&probe_out)
        .unwrap_or_else(|e| panic!("the probe script never ran ({e}): {transcript}"));
    assert!(
        probe.contains("aida=0"),
        "the aida commands failed: {probe}\n{transcript}"
    );

    // Inside the write set: the store's objects directory.
    assert!(
        store_probe.exists(),
        "<main>/.aida-store/objects must be writable: {probe}\n{transcript}"
    );
    std::fs::remove_file(&store_probe).unwrap();
    // Outside it: the store's .git pointer (denyWrite), and the rest of main.
    for (path, before) in unchanged.iter().zip(&before) {
        assert_eq!(
            &read(path),
            before,
            "{} must not be writable from a contained session: {probe}",
            path.display()
        );
    }

    // E1: the plant landed, and the host's store sync refuses it without
    // running its hook.
    // trace:BUG-1929 | ai:claude
    assert!(
        probe.lines().any(|l| l.trim() == "planted"),
        "the embedded-repository plant did not land: {probe}\n{transcript}"
    );
    let sync = Command::new(&aida)
        .args(["db", "sync"])
        .current_dir(&main)
        .output()
        .expect("spawn aida db sync");
    let sync_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&sync.stdout),
        String::from_utf8_lossy(&sync.stderr)
    );
    assert!(
        !marker.exists(),
        "a host-side store sync ran the planted fsmonitor: {sync_text}\n{probe}"
    );
    assert!(
        !sync.status.success() && sync_text.contains("embedded git repository"),
        "the host store sync must refuse the planted repository: {sync_text}\n{probe}"
    );

    let status_is = |text: &str, want: &str| {
        text.lines()
            .any(|l| l.trim_start().starts_with("status:") && l.contains(want))
    };
    let shown = run(&main, &aida, &["show", "BUG-1", "--full", "--comments"]);
    assert!(
        status_is(&shown, "in-progress"),
        "the claim must be visible from main: {shown}\n{transcript}"
    );
    assert!(
        shown.contains("contained-ok"),
        "the comment must be visible from main: {shown}\n{transcript}"
    );
    let done = run(&main, &aida, &["show", "BUG-2"]);
    assert!(
        status_is(&done, "done"),
        "queue done must be visible from main: {done}\n{transcript}"
    );

    assert!(
        !marker.exists(),
        "a host-side aida read ran the planted fsmonitor: {probe}"
    );
    // Remove the plant (the host's own raw git below would run its hook).
    let _ = Command::new("git")
        .args(["rm", "-q", "--cached", "--ignore-unmatch", "objects/zz"])
        .current_dir(&store)
        .output();
    for planted in [evil_sub, store.join(".evil.git")] {
        let _ = std::fs::remove_dir_all(planted);
    }
    let _ = std::fs::remove_file(&evil_hook);

    // The writes are committed to the store, not left in its working tree.
    let log = run(
        &store,
        "git",
        &[
            "log",
            "--format=%s",
            &format!("{}..HEAD", store_head_before.trim()),
        ],
    );
    assert!(
        !log.trim().is_empty(),
        "the contained writes must commit to the store: {log}\n{transcript}"
    );
    let dirty = run(
        &store,
        "git",
        &["status", "--porcelain", "--", "objects", "registry/queues"],
    );
    assert!(
        dirty.trim().is_empty(),
        "contained writes left uncommitted in the store: {dirty}"
    );

    // F1: give the store an origin whose history carries a gitlink at the
    // plant, merge it, then move the gitlink on origin. The host's store fetch
    // must bring the new tip without running the plant's `uploadpack`. The
    // fixture's own git runs carry the hardening flags so only aida's run is
    // under test.
    // trace:BUG-1929 | ai:claude
    assert!(
        probe.lines().any(|l| l.trim() == "f1-planted"),
        "the uploadpack plant did not land: {probe}\n{transcript}"
    );
    let hg = |dir: &Path, args: &[&str]| -> String {
        let mut all = vec![
            "-c",
            "core.fsmonitor=false",
            "-c",
            "submodule.recurse=false",
            "-c",
            "fetch.recurseSubmodules=false",
            "-c",
            "diff.ignoreSubmodules=all",
            "-c",
            "user.email=f1@example.com",
            "-c",
            "user.name=f1",
        ];
        all.extend_from_slice(args);
        run(dir, "git", &all)
    };
    let origin_s = f1_origin.to_str().unwrap();
    let seed = parent.join("f1-seed");
    hg(&parent, &["init", "-q", "--bare", origin_s]);
    hg(&main, &["remote", "add", "origin", origin_s]);
    hg(
        &store,
        &["push", "-q", "origin", "HEAD:refs/heads/aida-store"],
    );
    hg(
        &parent,
        &[
            "clone",
            "-q",
            "-b",
            "aida-store",
            origin_s,
            seed.to_str().unwrap(),
        ],
    );
    let move_gitlink = |msg: &str| -> String {
        let head = hg(&seed, &["rev-parse", "HEAD"]).trim().to_string();
        hg(
            &seed,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{head},f1-sub"),
            ],
        );
        hg(&seed, &["commit", "-q", "-m", msg]);
        hg(&seed, &["push", "-q", "origin", "aida-store"]);
        hg(&seed, &["rev-parse", "HEAD"]).trim().to_string()
    };
    std::fs::write(
        seed.join(".gitmodules"),
        "[submodule \"f1\"]\n\tpath = f1-sub\n\turl = ./f1\n",
    )
    .unwrap();
    hg(&seed, &["add", ".gitmodules"]);
    move_gitlink("f1: gitlink");
    hg(&store, &["fetch", "-q", "origin", "aida-store"]);
    hg(&store, &["merge", "-q", "--ff-only", "origin/aida-store"]);
    let tip = move_gitlink("f1: move gitlink");
    let fetch = Command::new(&aida)
        .args(["fetch", "--store-only"])
        .current_dir(&main)
        .output()
        .expect("spawn aida fetch");
    let fetch_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&fetch.stdout),
        String::from_utf8_lossy(&fetch.stderr)
    );
    assert!(
        !up_marker.exists(),
        "a host-side store fetch ran the planted uploadpack: {fetch_text}\n{probe}"
    );
    let fetched = hg(&store, &["rev-parse", "origin/aida-store"]);
    assert_eq!(
        fetched.trim(),
        tip,
        "the host store fetch must still bring the new tip: {fetch_text}"
    );
    // The plant is live: an unhardened fetch over the next gitlink move runs it.
    move_gitlink("f1: move gitlink again");
    let _ = Command::new("git")
        .args(["fetch", "-q", "origin", "aida-store"])
        .current_dir(&store)
        .output();
    assert!(
        up_marker.exists(),
        "control: a raw fetch should have run the planted uploadpack: {probe}"
    );
}

/// Line indexes of `src` that are test code: every line of an inline
/// `#[cfg(test)] mod name {` block, up to its closing `}` in column 0.
fn bug_1929_test_lines(lines: &[&str]) -> std::collections::HashSet<usize> {
    let module = regex::Regex::new(r"^\s*(pub(\(crate\))? )?mod \w+\s*\{").unwrap();
    let mut skip = std::collections::HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]" && lines.get(i + 1).is_some_and(|l| module.is_match(l))
        {
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "}" {
                skip.insert(j);
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    skip
}

/// The raw `Command::new("git")` sites in `src` (1-based lines) whose builder
/// chain, up to its first `;` and at most 12 lines, names a store path, and
/// that carry no `raw-git-ok(BUG-1929): <why>` comment in the three lines
/// above. With `any_raw`, every raw site counts, store-named or not.
fn bug_1929_raw_store_git(src: &str, any_raw: bool) -> Vec<usize> {
    let raw = regex::Regex::new(r#"Command::new\("git"\)"#).unwrap();
    let store = regex::Regex::new(
        r"\b(store|store_path|store_root|store_dir|store_worktree|store_full|aida_repo)\b|\.aida-store",
    )
    .unwrap();
    let lines: Vec<&str> = src.lines().collect();
    let tests = bug_1929_test_lines(&lines);
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if tests.contains(&i) || line.trim_start().starts_with("//") || !raw.is_match(line) {
            continue;
        }
        let chain = lines[i..lines.len().min(i + 12)].join(" ");
        let chain = chain.split(';').next().unwrap_or_default();
        let justified = lines[i.saturating_sub(3)..i]
            .iter()
            .any(|l| l.contains("raw-git-ok(BUG-1929):"));
        if (any_raw || store.is_match(chain)) && !justified {
            found.push(i + 1);
        }
    }
    found
}

// F1 guard: every host git command over the store starts at
// `aida_core::git_ops::host_git_command`, which carries the hardening
// (`fetch.recurseSubmodules=false`, `submodule.recurse=false`, ...). Scans the
// non-test source of aida-core and aida-cli-lib for a raw `Command::new("git")`
// whose builder names a store path; `store_push_pull.rs`, which runs both the
// code and the store legs through shared helpers, may hold no raw git call at
// all. A read-only call that names a store ref but runs in the code checkout
// may stay raw with a `raw-git-ok(BUG-1929): <why>` comment just above it.
// trace:BUG-1929 | ai:claude
#[test]
fn bug_1929_store_git_guard() {
    // The scanner itself: a raw store call is caught, a justified one is not,
    // and test modules are skipped.
    let sample = "fn a(store_path: &Path) {\n    let _ = std::process::Command::new(\"git\")\n        \
                  .args([\"fetch\"])\n        .current_dir(store_path)\n        .output();\n}\n\
                  fn b(store_path: &Path) {\n    // raw-git-ok(BUG-1929): read-only.\n    \
                  let _ = Command::new(\"git\").arg(store_path).output();\n}\n\
                  #[cfg(test)]\nmod t {\n    fn c(store: &Path) { Command::new(\"git\").arg(store); }\n}\n";
    assert_eq!(bug_1929_raw_store_git(sample, false), vec![2]);

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut offenders = Vec::new();
    for crate_src in ["aida-core/src", "aida-cli-lib/src"] {
        let mut stack = vec![workspace.join(crate_src)];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if path.is_dir() {
                    if name != "tests" {
                        stack.push(path);
                    }
                    continue;
                }
                if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).unwrap();
                let any_raw = name == "store_push_pull.rs";
                for line in bug_1929_raw_store_git(&src, any_raw) {
                    let rel = path.strip_prefix(workspace).unwrap_or(&path);
                    offenders.push(format!("{}:{line}", rel.display()));
                }
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "raw `Command::new(\"git\")` over the store; build it with \
         `aida_core::git_ops::host_git_command` (or justify a read-only call in the code \
         checkout with `// raw-git-ok(BUG-1929): <why>`): {offenders:?}"
    );
}

/// Run git in `dir` with a fixed identity; panics on failure.
fn bug_1929_git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// F1 fixture under `dir`: a bare `origin` whose `aida-store` branch commits a
/// gitlink `objects/zz` plus `.gitmodules` (as an older binary or a direct
/// push could), a store `<dir>/.aida-store` cloned from it, a newer origin
/// commit that moves the gitlink, and an embedded repository planted at
/// `<store>/objects/zz/.git` the way a contained session could, whose own
/// config sets `remote.origin.uploadpack` to a script that appends to
/// `marker`. A fetch that recursed into the submodule would run the script.
/// Returns (store, marker, the origin's new tip).
// trace:BUG-1929 | ai:claude
fn bug_1929_f1_fixture(dir: &Path) -> (PathBuf, PathBuf, String) {
    std::fs::create_dir_all(dir).unwrap();
    let origin = dir.join("origin.git");
    bug_1929_git(dir, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    let seed = dir.join("seed");
    bug_1929_git(
        dir,
        &["init", "-q", "-b", "aida-store", seed.to_str().unwrap()],
    );
    std::fs::create_dir_all(seed.join("objects")).unwrap();
    std::fs::write(seed.join("objects/a.yaml"), "id: A\n").unwrap();
    bug_1929_git(&seed, &["add", "objects/a.yaml"]);
    bug_1929_git(&seed, &["commit", "-qm", "init"]);
    std::fs::write(
        seed.join(".gitmodules"),
        "[submodule \"zz\"]\n\tpath = objects/zz\n\turl = ./zz\n",
    )
    .unwrap();
    bug_1929_git(&seed, &["add", ".gitmodules"]);
    let link = |seed: &Path| {
        let head = bug_1929_git(seed, &["rev-parse", "HEAD"]);
        let info = format!("160000,{head},objects/zz");
        bug_1929_git(seed, &["update-index", "--add", "--cacheinfo", &info]);
    };
    link(&seed);
    bug_1929_git(&seed, &["commit", "-qm", "gitlink"]);
    bug_1929_git(
        &seed,
        &["push", "-q", origin.to_str().unwrap(), "aida-store"],
    );

    let store = dir.join(".aida-store");
    bug_1929_git(
        dir,
        &[
            "clone",
            "-q",
            "-b",
            "aida-store",
            origin.to_str().unwrap(),
            store.to_str().unwrap(),
        ],
    );
    std::fs::write(seed.join("objects/b.yaml"), "id: B\n").unwrap();
    bug_1929_git(&seed, &["add", "objects/b.yaml"]);
    link(&seed);
    bug_1929_git(&seed, &["commit", "-qm", "move the gitlink"]);
    bug_1929_git(
        &seed,
        &["push", "-q", origin.to_str().unwrap(), "aida-store"],
    );
    let tip = bug_1929_git(&seed, &["rev-parse", "HEAD"]);

    let marker = dir.join("uploadpack-ran");
    let script = dir.join("evil-uploadpack.sh");
    crate::test_exec::write_executable(
        &script,
        format!("#!/bin/sh\necho ran >> '{}'\nexit 1\n", marker.display()),
    );
    let sub = store.join("objects/zz");
    std::fs::create_dir_all(&sub).unwrap();
    bug_1929_git(&sub, &["init", "-q"]);
    for (key, value) in [
        ("remote.origin.url", origin.to_str().unwrap()),
        ("remote.origin.uploadpack", script.to_str().unwrap()),
        ("protocol.file.allow", "always"),
    ] {
        bug_1929_git(&sub, &["config", key, value]);
    }
    (store, marker, tip)
}

// F1 regression: the store fetch legs never recurse into a planted embedded
// repository. On 04b529c167 `fetch_branch` and `refresh_remote_branch_state`
// ran a raw `git fetch` that recursed on demand into `objects/zz` and ran its
// `uploadpack` on the host.
// trace:BUG-1929 | ai:claude
#[test]
fn bug_1929_f1_store_fetch_never_runs_a_planted_uploadpack() {
    let tmp = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();

    // The plant is live: an unhardened fetch runs it.
    let (store, marker, _) = bug_1929_f1_fixture(&base.join("raw"));
    let _ = std::process::Command::new("git")
        .args(["fetch", "-q", "origin", "aida-store"])
        .current_dir(&store)
        .output()
        .unwrap();
    assert!(
        marker.exists(),
        "precondition: a plain git fetch must run the planted uploadpack"
    );

    // `aida fetch` / `aida pull`: the store fetch leg.
    let (store, marker, tip) = bug_1929_f1_fixture(&base.join("fetch"));
    crate::store_push_pull::fetch_branch(&store, "aida-store", true).unwrap();
    assert!(
        !marker.exists(),
        "the store fetch leg ran the planted uploadpack on the host"
    );
    assert_eq!(
        bug_1929_git(&store, &["rev-parse", "origin/aida-store"]),
        tip
    );

    // `aida push`: the plan's bounded refresh of origin/aida-store.
    let (store, marker, tip) = bug_1929_f1_fixture(&base.join("refresh"));
    let fresh = crate::store_push_pull::refresh_remote_branch_state(
        &store,
        "aida-store",
        std::time::Duration::from_secs(60),
    );
    assert_eq!(fresh, crate::store_push_pull::RemoteBranchFreshness::Fresh);
    assert!(
        !marker.exists(),
        "the push plan's store refresh ran the planted uploadpack on the host"
    );
    assert_eq!(
        bug_1929_git(&store, &["rev-parse", "origin/aida-store"]),
        tip
    );

    // The auto-push leg refuses the store outright: it holds an embedded
    // repository.
    let (store, marker, _) = bug_1929_f1_fixture(&base.join("push"));
    let head = bug_1929_git(&store, &["rev-parse", "HEAD"]);
    let err = aida_core::git_ops::refuse_embedded_repos_for_push(&store, &head)
        .unwrap_err()
        .to_string();
    assert!(err.contains("embedded git repository"), "{err}");
    assert!(!marker.exists());
}

// The primary checkout can itself be the code write root. Its verified host
// hooks remain read-only for sandboxed Bash AND direct file-writing tools.
// trace:BUG-1929 | ai:codex
#[test]
fn bug_1929_primary_root_never_grants_host_hook_writes() {
    let (_tmp, main) = main_fixture();
    let hooks = main.join(".git/aida-store-hooks-test");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("commit-msg");
    std::fs::write(&hook, "trusted hook").unwrap();
    let settings = settings_with_store(&main, &main);
    assert!(!sandbox_may_write(&settings, &hook), "{settings}");
    assert!(
        bound_covers(&deny_write(&settings), &hook),
        "explicit host authority deny is required: {settings}"
    );
    let denies = settings["permissions"]["deny"].as_array().unwrap();
    for tool in WRITE_TOOLS {
        assert!(
            denies.contains(&serde_json::json!(format!(
                "{tool}(/{}/**)",
                main.join(".git").display()
            ))),
            "{settings}"
        );
        assert!(
            denies.contains(&serde_json::json!(format!(
                "{tool}(/{})",
                main.join(".aida-store/.git").display()
            ))),
            "pointer must be protected: {settings}"
        );
    }
    assert_eq!(
        allow_write(&settings),
        sandbox_allow_write_entries(&contained_store_write_paths(&main))
    );
}
