//! BUG-1928: the contained write boundary, exercised against REAL paths.
//!
//! Which test is which:
//! - Harness (always runs): the generated `--settings` JSON is fed to
//!   `evaluator::allowed`, a small implementation of Claude Code's documented
//!   Read/Edit rule matching, over real tempdir paths.
//! - Real claude (env-gated, `AIDA_TEST_REAL_CLAUDE=1`):
//!   `real_claude_contained_session_write_boundary` drives `claude -p
//!   --permission-mode dontAsk` with the generated settings and checks the
//!   filesystem afterwards.
//!
//! trace:BUG-1928 | ai:claude

use super::evaluator::{allowed, anchor, normalize, parse_rule};
use super::*;
use std::path::{Path, PathBuf};

/// `<tmp>/aida-pool-aida-relay-0`, a linked-worktree-shaped directory (a `.git`
/// FILE, as `git worktree add` leaves) next to where the main checkout and the
/// never-created requested path would sit.
fn pool_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let worktree = parent.join("aida-pool-aida-relay-0");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: /nonexistent/aida-relay/.git/worktrees/aida-pool-aida-relay-0\n",
    )
    .unwrap();
    (tmp, parent, worktree)
}

fn settings_for(worktree: &Path) -> serde_json::Value {
    let json =
        crate::session::contained_settings_json(&[], false, &contained_write_roots(worktree));
    serde_json::from_str(&json).unwrap()
}

/// Perform the write only when the evaluator says the contained session may.
fn attempt_write(
    settings: &serde_json::Value,
    tool: &str,
    path: &Path,
    settings_root: &Path,
    home: &Path,
) -> bool {
    if !allowed(settings, tool, path, settings_root, home) {
        return false;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, "ok").unwrap();
    true
}

// BEHAVIOURAL (harness): a contained session can write inside its worktree and
// is denied one level above it, for every file-writing tool, whatever the
// settings root is taken to be (the rules are absolute).
#[test]
fn contained_session_writes_inside_worktree_denied_one_level_above() {
    let (_tmp, parent, worktree) = pool_fixture();
    let settings = settings_for(&worktree);
    let home = parent.join("home");
    let main_checkout = parent.join("aida-relay");
    for settings_root in [worktree.as_path(), main_checkout.as_path()] {
        for tool in ["Edit", "Write", "MultiEdit"] {
            let inside = worktree.join("src").join(format!("{tool}-inside.rs"));
            assert!(
                attempt_write(&settings, tool, &inside, settings_root, &home),
                "{tool} inside the worktree must be allowed: {settings}"
            );
            assert!(
                inside.is_file(),
                "{tool} wrote nothing at {}",
                inside.display()
            );

            let above = parent.join(format!("{tool}-outside.txt"));
            assert!(
                !attempt_write(&settings, tool, &above, settings_root, &home),
                "{tool} one level above the worktree must be denied: {settings}"
            );
            assert!(!above.exists(), "{tool} escaped to {}", above.display());

            // `..` out of the worktree is the same parent directory.
            let dotdot = worktree.join("..").join(format!("{tool}-dotdot.txt"));
            assert!(
                !attempt_write(&settings, tool, &dotdot, settings_root, &home),
                "{tool} via `..` must be denied"
            );
            assert!(!parent.join(format!("{tool}-dotdot.txt")).exists());

            // A sibling sharing the worktree's name as a prefix is outside.
            let sibling = parent
                .join("aida-pool-aida-relay-0-evil")
                .join(format!("{tool}.txt"));
            assert!(
                !attempt_write(&settings, tool, &sibling, settings_root, &home),
                "{tool} in a prefix-sharing sibling must be denied"
            );
            assert!(!sibling.exists());
        }
        // NotebookEdit follows the same boundary.
        assert!(allowed(
            &settings,
            "NotebookEdit",
            &worktree.join("nb.ipynb"),
            settings_root,
            &home
        ));
        assert!(!allowed(
            &settings,
            "NotebookEdit",
            &parent.join("nb.ipynb"),
            settings_root,
            &home
        ));
    }
}

// No allow rule is broader than the worktree: every rule is absolute (`//`) and
// anchors exactly at the worktree, whatever settings root / home is assumed.
#[test]
fn no_allow_rule_is_broader_than_the_worktree() {
    let (_tmp, _parent, worktree) = pool_fixture();
    let settings = settings_for(&worktree);
    let allow: Vec<&str> = settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str())
        .collect();
    assert!(
        !allow.contains(&"Edit(/**)"),
        "the BUG-1928 rule is gone: {allow:?}"
    );
    let mut tools: Vec<&str> = Vec::new();
    let mut expected = normalize(&worktree).unwrap();
    expected.push("**".to_string());
    for rule in &allow {
        let (tool, spec) = parse_rule(rule).expect("Tool(spec) shape");
        assert!(spec.starts_with("//"), "rule must be absolute: {rule}");
        for (root, home) in [("/unrelated/root", "/unrelated/home"), ("/", "/")] {
            let pattern = anchor(spec, Path::new(root), Path::new(home)).unwrap();
            assert_eq!(pattern, expected, "{rule} must anchor at the worktree");
        }
        tools.push(tool);
    }
    tools.sort_unstable();
    let mut want = WRITE_TOOLS.to_vec();
    want.sort_unstable();
    assert_eq!(tools, want, "one rule per file-writing tool");
    // The deny list stays Bash-only, so it never shadows a worktree write.
    for rule in settings["permissions"]["deny"].as_array().unwrap() {
        assert!(rule.as_str().unwrap().starts_with("Bash("), "{rule}");
    }
}

// Regression: the pre-BUG-1928 `Edit(/**)` is anchored at the project root, so
// resolved against the main checkout it does not cover the warm-pool worktree
// the drain actually ran in. (Against real claude 2.1.296 it did not even
// cover the session's own cwd — see the module docs.)
#[test]
fn legacy_single_slash_rule_misses_a_pool_worktree() {
    let (_tmp, parent, worktree) = pool_fixture();
    let legacy = serde_json::json!({"permissions": {"allow": ["Edit(/**)"]}});
    let main_checkout = parent.join("aida-relay");
    let target = worktree.join("src").join("lib.rs");
    assert!(!allowed(&legacy, "Edit", &target, &main_checkout, &parent));
    assert!(!allowed(&legacy, "Write", &target, &main_checkout, &parent));
    // The new rules cover it from the same settings root.
    assert!(allowed(
        &settings_for(&worktree),
        "Write",
        &target,
        &main_checkout,
        &parent
    ));
}

// Fail closed: a root that cannot be expressed exactly emits no rule at all.
#[test]
fn unsafe_roots_emit_no_rule() {
    for root in [
        "relative/worktree",
        "/",
        "/tmp/a*b",
        "/tmp/a?b",
        "/tmp/[ab]",
        "/tmp/a(b)",
        "/tmp/a{b}",
        "/tmp/a\\b",
        "/tmp/a/../b",
    ] {
        let roots = vec![PathBuf::from(root)];
        assert!(
            write_allow_rules(&roots).is_empty(),
            "`{root}` must not produce a rule"
        );
        assert!(
            write_boundary_announcement(&roots).starts_with("write boundary: none"),
            "`{root}`"
        );
    }
    // Empty roots: no allow rule, so dontAsk denies every edit (never wider).
    let json = crate::session::contained_settings_json(&[], false, &[]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["permissions"]["allow"], serde_json::json!([]));
}

// The boundary is derived from the worktree the session actually runs in. Feed
// a pool worktree: the root discovered from the session's cwd, the announced
// phase-1 workspace, and the rule path are one and the same, and the requested
// but never-created `<repo>-bug-81` path appears nowhere.
#[test]
fn pool_worktree_announcement_matches_rule_path() {
    let (_tmp, parent, worktree) = pool_fixture();
    let requested = parent.join("aida-relay-bug-81");
    assert!(!requested.exists());

    // The headless spawn derives its root from cwd (`headless_worktree_root`);
    // a linked worktree resolves to itself.
    let session_root = crate::find_project_root_from(&worktree.join("src")).unwrap();
    assert_eq!(session_root, worktree);
    let rules = write_allow_rules(&contained_write_roots(&session_root));

    let line = workspace_announcement(
        launched_workspace_label(Some("1")),
        &worktree,
        "bug-81-work",
    );
    let announced = line
        .strip_prefix("phase 1 workspace: ")
        .and_then(|rest| rest.strip_suffix(" [bug-81-work]"))
        .expect("announcement shape");
    assert_eq!(announced, worktree.display().to_string());

    let expected: Vec<String> = WRITE_TOOLS
        .iter()
        .map(|tool| format!("{tool}(/{announced}/**)"))
        .collect();
    assert_eq!(
        rules, expected,
        "announced workspace and rule path must be identical"
    );
    assert_eq!(
        write_boundary_announcement(&contained_write_roots(&worktree)),
        format!("write boundary: /{announced}/**")
    );
    let requested_str = requested.display().to_string();
    assert!(rules.iter().all(|r| !r.contains(&requested_str)));

    // The full launch flags carry the same rules.
    let flags = crate::session::claude_contained_flags_for(&worktree);
    let pos = flags.iter().position(|f| f == "--settings").unwrap();
    let v: serde_json::Value = serde_json::from_str(&flags[pos + 1]).unwrap();
    let allow: Vec<String> = v["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str().map(str::to_string))
        .collect();
    assert_eq!(allow, expected);
}

#[test]
fn workspace_label_names_phase_one_only_for_the_phase_one_child() {
    assert_eq!(launched_workspace_label(Some("1")), "phase 1 workspace");
    assert_eq!(launched_workspace_label(Some(" 1\n")), "phase 1 workspace");
    assert_eq!(launched_workspace_label(Some("3")), "workspace");
    assert_eq!(launched_workspace_label(None), "workspace");
}

// REAL CLAUDE (env-gated): drive `claude -p --permission-mode dontAsk` with the
// generated contained settings from inside a tempdir worktree. Costs a model
// call, so CI skips it unless `AIDA_TEST_REAL_CLAUDE=1`. Optional:
// `AIDA_TEST_CLAUDE_BIN` (default `claude`), `AIDA_TEST_REAL_CLAUDE_MODEL`
// (default `haiku`).
#[test]
fn real_claude_contained_session_write_boundary() {
    if std::env::var("AIDA_TEST_REAL_CLAUDE").ok().as_deref() != Some("1") {
        eprintln!("skipping: set AIDA_TEST_REAL_CLAUDE=1 to drive a real `claude -p`");
        return;
    }
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let worktree = parent.join("aida-pool-fixture-0");
    std::fs::create_dir_all(&worktree).unwrap();
    let git = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&worktree)
        .status()
        .unwrap();
    assert!(git.success());

    let settings =
        crate::session::contained_settings_json(&[], false, &contained_write_roots(&worktree));
    let inside = worktree.join("inside.txt");
    let outside = parent.join("outside.txt");
    let prompt = format!(
        "Use the Write tool to create the file {} with the content ok. Then use the Write \
         tool to create the file {} with the content bad. Use no other tool, and do not \
         retry a denied call another way.",
        inside.display(),
        outside.display()
    );
    let program = std::env::var("AIDA_TEST_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string());
    let model =
        std::env::var("AIDA_TEST_REAL_CLAUDE_MODEL").unwrap_or_else(|_| "haiku".to_string());
    // The prompt goes on stdin: `--disallowed-tools` is variadic.
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
            "--disallowed-tools",
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
    assert_eq!(
        std::fs::read_to_string(&inside)
            .ok()
            .map(|s| s.trim().to_string())
            .as_deref(),
        Some("ok"),
        "the write inside the worktree must land: {transcript}"
    );
    assert!(
        !outside.exists(),
        "the write one level above must be denied: {transcript}"
    );
}
