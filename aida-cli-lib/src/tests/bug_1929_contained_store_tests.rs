//! BUG-1929: the main checkout's store and cache in the contained write set.
//!
//! Which test is which:
//! - Unit: the generated `sandbox.filesystem.allowWrite` is exactly the store
//!   directory and the cache files, nothing else in the main checkout.
//! - Harness (always runs): a small model of how Claude Code's Linux sandbox
//!   applies `allowWrite` (each existing entry bound as is, no glob expansion),
//!   checked over real tempdir paths.
//! - Real claude (env-gated, `AIDA_TEST_REAL_CLAUDE=1`):
//!   `real_claude_contained_session_claims_and_comments` runs `aida` inside a
//!   contained session in a linked worktree and reads the result from main.
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
    settings
        .pointer("/sandbox/filesystem/allowWrite")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// How Claude Code's Linux sandbox applies `allowWrite` (claude 2.1.296): a
/// `//p` entry is the absolute path `/p`; it is bound only when it exists at
/// launch, with no glob expansion; a directory bind covers everything under it.
fn sandbox_may_write(entries: &[String], target: &Path) -> bool {
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
    let entries = allow_write(&settings_with_store(&worktree, &main));

    for allowed in [
        main.join(".aida-store/objects/BUG-1.yaml"),
        main.join(".aida-store/.aida/store-write.lock"),
        main.join(".aida/cache.db"),
        main.join(".aida/cache.db-wal"),
        main.join(".aida/cache.db-shm"),
    ] {
        assert!(
            sandbox_may_write(&entries, &allowed),
            "{}",
            allowed.display()
        );
    }
    for denied in [
        main.join(".aida/config.toml"),
        main.join(".aida/cache.db.bak"),
        main.join(".aida/agents.toml"),
        main.join(".git/config"),
        main.join("src/lib.rs"),
        main.clone(),
    ] {
        assert!(
            !sandbox_may_write(&entries, &denied),
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
// contained `claude -p` session runs `aida edit --status in-progress` and
// `aida comment add` through sandboxed Bash; both must be visible from the
// main checkout. Costs a model call, so CI skips it unless
// `AIDA_TEST_REAL_CLAUDE=1`. Optional: `AIDA_TEST_CLAUDE_BIN` (default
// `claude`), `AIDA_TEST_AIDA_BIN` (default `aida`),
// `AIDA_TEST_REAL_CLAUDE_MODEL` (default `haiku`).
#[test]
fn real_claude_contained_session_claims_and_comments() {
    if std::env::var("AIDA_TEST_REAL_CLAUDE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set AIDA_TEST_REAL_CLAUDE=1 to drive a real `claude -p`");
        return;
    }
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

    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let main = parent.join("main");
    std::fs::create_dir_all(&main).unwrap();
    run(&main, "git", &["init", "-q", "-b", "main"]);
    run(&main, "git", &["config", "user.email", "test@example.com"]);
    run(&main, "git", &["config", "user.name", "Test"]);
    std::fs::write(main.join("README.md"), "fixture\n").unwrap();
    run(&main, "git", &["add", "README.md"]);
    run(&main, "git", &["commit", "-q", "-m", "init"]);
    run(&main, &aida, &["init", "--no-skills", "--no-hooks"]);
    let added = run(
        &main,
        &aida,
        &[
            "add",
            "--title",
            "Contained fixture",
            "--type",
            "bug",
            "--status",
            "approved",
        ],
    );
    let id = added
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|w| {
            w.len() > 4 && w.starts_with("BUG-") && w[4..].chars().all(|c| c.is_ascii_digit())
        })
        .unwrap_or_else(|| panic!("no BUG id in: {added}"))
        .to_string();

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
    aida_core::db::prime_cache_sidecars(&main.join(".aida/cache.db"));
    let settings = crate::session::contained_settings_json_with_store(
        &[],
        false,
        &contained_write_roots(&worktree),
        &contained_store_write_paths(&main),
    );
    let prompt = format!(
        "Run exactly this one Bash command and nothing else: \
         {aida} edit {id} --status in-progress && {aida} comment add {id} contained-ok"
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

    let shown = run(&main, &aida, &["show", &id, "--full", "--comments"]);
    assert!(
        shown.contains("in-progress"),
        "the claim must be visible from main: {shown}\n{transcript}"
    );
    assert!(
        shown.contains("contained-ok"),
        "the comment must be visible from main: {shown}\n{transcript}"
    );
}
