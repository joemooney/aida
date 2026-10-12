//! BUG-1929: the main checkout's store and cache in the contained write set.
//!
//! Which test is which:
//! - Unit: the generated `sandbox.filesystem.allowWrite` is exactly the store
//!   directory and the cache files, nothing else in the main checkout, and
//!   `denyWrite` is exactly the store's `.git` pointer.
//! - Harness (always runs): a small model of how Claude Code's Linux sandbox
//!   applies `allowWrite` and `denyWrite` (each existing entry bound as is, no
//!   glob expansion, deny over allow), checked over real tempdir paths.
//! - Real claude (env-gated, `AIDA_TEST_REAL_CLAUDE=1`):
//!   `real_claude_contained_session_claims_and_comments` runs `aida` inside a
//!   contained session in a linked worktree, probes writes inside and outside
//!   the write set, plants an embedded repository in the store (E1) and
//!   checks the host's store sync refuses it without running its
//!   `core.fsmonitor`, and reads the result from main. Its fixture lives under
//!   the operator's real `~/.cache`, outside every sandbox-writable temp
//!   directory.
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
        vec![format!("/{}", main.join(".aida-store/.git").display())],
        "the store's .git pointer is denied, nothing under <main>/.git is named"
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
        vec![link.join(".aida-store/.git"), main.join(".aida-store/.git")],
        "each spelling of the store gets its .git pointer denied"
    );
    let settings = crate::session::contained_settings_json_with_store(&[], false, &[], &paths);
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(deny_write(&settings).len(), 2, "{settings}");
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
}
