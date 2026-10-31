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

/// `<tmp>/aida-pool-aida-relay-0`, a linked worktree (a `.git` FILE, as `git
/// worktree add` leaves, whose gitdir `<tmp>/aida-relay/.git/worktrees/<name>`
/// carries a `commondir` back to `<tmp>/aida-relay/.git`) next to where the
/// main checkout and the never-created requested path would sit.
// trace:TASK-1952 | trace:TASK-2005 | ai:antigravity
fn pool_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let worktree = parent.join("aida-pool-aida-relay-0");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    let wt_gitdir = parent.join("aida-relay/.git/worktrees/aida-pool-aida-relay-0");
    std::fs::create_dir_all(&wt_gitdir).unwrap();
    std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", wt_gitdir.display()),
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
        .filter(|r| is_write_rule(r))
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
    // The deny list stays Bash-only apart from the worktree's own `.git`
    // pointer (TASK-1989), so it never shadows a worktree source write.
    // trace:TASK-1952 | trace:TASK-1989 | ai:claude
    let pointer = format!("/{}", worktree.join(".git").display());
    for rule in settings["permissions"]["deny"].as_array().unwrap() {
        let rule = rule.as_str().unwrap();
        assert!(
            rule.starts_with("Bash(")
                || parse_rule(rule).is_some_and(|(_, spec)| {
                    spec == pointer || spec == format!("{pointer}/**")
                }),
            "{rule}"
        );
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
        "/work/a*b",
        "/work/a?b",
        "/work/[ab]",
        "/work/a(b)",
        "/work/a{b}",
        "/work/a\\b",
        "/work/a/../b",
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
    let flags = crate::session::claude_contained_flags_for(&worktree).unwrap();
    let pos = flags.iter().position(|f| f == "--settings").unwrap();
    let v: serde_json::Value = serde_json::from_str(&flags[pos + 1]).unwrap();
    let allow: Vec<String> = v["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str().map(str::to_string))
        .filter(|r| is_write_rule(r))
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
    // BUG-1971: the lib test binary redirects HOME to a temp dir, where claude
    // has no credentials. Give the claude child (only) the operator's real
    // home from the test fence. trace:BUG-1971 | ai:claude
    let real_home =
        crate::test_home::operator_real_home().expect("the test HOME fence names a real home");
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
        .env("HOME", real_home)
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

/// The write-tool `permissions.allow` rules in a contained flag set's
/// `--settings` JSON (BUG-2201's baseline Bash/Read rules are not the write
/// boundary, so they are left out).
fn allow_rules_in_flags(flags: &[String]) -> Vec<String> {
    let pos = flags.iter().position(|f| f == "--settings").unwrap();
    let v: serde_json::Value = serde_json::from_str(&flags[pos + 1]).unwrap();
    v["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str().map(str::to_string))
        .filter(|r| is_write_rule(r))
        .collect()
}

/// BUG-050: the argv's `--settings` (the uncontained headless posture alone)
/// carries no `permissions.allow` rules at all: no contained boundary.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
fn no_allow_rules(flags: &[String]) -> bool {
    let pos = flags.iter().position(|f| f == "--settings").unwrap();
    let v: serde_json::Value = serde_json::from_str(&flags[pos + 1]).unwrap();
    v["permissions"]["allow"]
        .as_array()
        .is_none_or(|a| a.is_empty())
}

/// Whether `rule` is one of the write boundary's `Tool(...)` rules.
// trace:BUG-2201 | ai:claude
fn is_write_rule(rule: &str) -> bool {
    WRITE_TOOLS
        .iter()
        .any(|tool| rule.starts_with(&format!("{tool}(")))
}

/// The roots named by `write boundary: //a/**, //b/**`, in rule-path form.
fn announced_roots(line: &str) -> Vec<String> {
    line.strip_prefix("write boundary: ")
        .expect("announcement shape")
        .split(", ")
        .map(|p| {
            p.strip_prefix('/')
                .and_then(|p| p.strip_suffix("/**"))
                .expect("pattern shape")
                .to_string()
        })
        .collect()
}

/// The roots the `Edit` rules in `allow` grant, in rule-path form.
fn settings_roots(allow: &[String]) -> Vec<String> {
    allow
        .iter()
        .filter_map(|r| r.strip_prefix("Edit(/"))
        .map(|r| r.strip_suffix("/**)").expect("rule shape").to_string())
        .collect()
}

// BUG-1971 (2): on a symlinked worktree path, the `queue work` boundary
// announcement and the contained `--settings` rules come from the same roots.
// trace:BUG-1971 | ai:claude
#[cfg(unix)]
#[test]
fn symlinked_worktree_announcement_matches_settings_roots() {
    let (_tmp, parent, worktree) = pool_fixture();
    let link = parent.join("linked-worktree");
    std::os::unix::fs::symlink(&worktree, &link).unwrap();
    let session_root = crate::find_project_root_from(&link.join("src")).unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(&session_root, None);

    let announced = announced_roots(&crate::session::contained_write_boundary_announcement());
    let in_settings = settings_roots(&allow_rules_in_flags(
        &crate::session::claude_contained_flags().unwrap(),
    ));
    assert_eq!(announced, in_settings);
    // Both spellings of the symlinked tree, and nothing else.
    let mut expected = vec![
        root_rule_path(&link).unwrap(),
        root_rule_path(&worktree).unwrap(),
    ];
    expected.dedup();
    assert_eq!(in_settings, expected);
}

/// A canonical temp dir that is not inside any git repository, or `None` when
/// the temp root itself sits in one (the test then has nothing to prove).
fn outside_git_dir() -> Option<(tempfile::TempDir, PathBuf)> {
    let tmp = tempfile::tempdir().unwrap();
    let outside = std::fs::canonicalize(tmp.path()).unwrap();
    if crate::find_project_root_from(&outside).is_ok() {
        eprintln!("skipping: the temp dir sits inside a git repository");
        return None;
    }
    Some((tmp, outside))
}

// BUG-1971 (3): outside a git repository the contained session has no write
// roots (it never falls back to cwd, which could be $HOME), so the settings
// allow no edit at all. Drives the PRODUCTION resolver through the cwd seam.
// trace:BUG-1971 | ai:claude
#[test]
fn outside_git_contained_session_has_no_write_roots() {
    let Some((_tmp, outside)) = outside_git_dir() else {
        return;
    };
    assert!(
        crate::session::contained_session_write_roots_from(&outside).is_empty(),
        "contained_session_write_roots_from must not fall back to cwd"
    );
    let _cwd = crate::session::contained_cwd_seam::pin(&outside);
    let roots = crate::session::contained_session_write_roots();
    assert!(roots.is_empty(), "outside git, no write roots: {roots:?}");
    let allow = allow_rules_in_flags(&crate::session::claude_contained_flags().unwrap());
    assert!(
        allow.iter().all(|r| !r.starts_with("Edit(")),
        "no Edit allow rule outside git: {allow:?}"
    );
    assert!(allow.is_empty(), "{allow:?}");
    // Nit 4: the announcement says why there is no boundary.
    let line = crate::session::contained_write_boundary_announcement();
    assert_eq!(line, OUTSIDE_GIT_BOUNDARY_ANNOUNCEMENT);
    assert!(line.contains("not inside a git repository"), "{line}");
    assert!(!line.contains("exact rule"), "{line}");
}

// BUG-1971 (1): inside a worktree the production resolver scopes the roots to
// the git root of cwd, not to cwd itself.
// trace:BUG-1971 | ai:claude
#[test]
fn contained_session_write_roots_are_the_git_root_of_cwd() {
    let (_tmp, _parent, worktree) = pool_fixture();
    let sub = worktree.join("src");
    assert_eq!(
        crate::session::contained_session_write_roots_from(&sub),
        contained_write_roots(&worktree)
    );
    let _cwd = crate::session::contained_cwd_seam::pin(&sub);
    assert_eq!(
        crate::session::contained_session_write_roots(),
        contained_write_roots(&worktree)
    );
}

// BUG-1971 (should-fix 2): the `queue work` launch lines announce the roots the
// launch's `--settings` use (git root of the process cwd), not the lease path
// as given — here a requested path the session does not run in.
// trace:BUG-1971 | ai:claude
#[test]
fn queue_work_boundary_line_uses_the_launch_roots_not_the_lease_path() {
    let (_tmp, parent, worktree) = pool_fixture();
    let requested = parent.join("aida-requested");
    std::fs::create_dir_all(&requested).unwrap();
    let _cwd = crate::session::contained_cwd_seam::pin(&worktree.join("src"));

    let lines = queue_work_launch_lines(None, &requested, "bug-1971", true);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("workspace: "), "{lines:?}");
    let in_settings = settings_roots(&allow_rules_in_flags(
        &crate::session::claude_contained_flags().unwrap(),
    ));
    assert_eq!(announced_roots(&lines[1]), in_settings);
    assert!(!lines[1].contains("aida-requested"), "{lines:?}");
    // Uncontained (or non-claude) launches print no boundary line.
    assert_eq!(
        queue_work_launch_lines(None, &requested, "bug-1971", false).len(),
        1
    );
}

// BUG-1971 (should-fix 3): a contained resume grants only when its cwd is
// itself a git root; a subdirectory of a checkout or a dir outside git
// (`$HOME`-like) yields no write roots.
// trace:BUG-1971 | ai:claude
#[test]
fn contained_resume_fails_closed_unless_cwd_is_a_git_root() {
    let (_tmp, _parent, worktree) = pool_fixture();
    assert_eq!(
        crate::session::contained_resume_write_roots(&worktree),
        contained_write_roots(&worktree)
    );
    let sub = worktree.join("src");
    assert!(crate::session::contained_resume_write_roots(&sub).is_empty());
    let args =
        crate::session::claude_headless_resume_args_with_posture("p", "sid", Some(&sub)).unwrap();
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-mode", "dontAsk"]),
        "{args:?}"
    );
    assert!(allow_rules_in_flags(&args).is_empty(), "{args:?}");
    if let Some((_tmp2, outside)) = outside_git_dir() {
        assert!(crate::session::contained_resume_write_roots(&outside).is_empty());
    }
}

// BUG-1971 (4): a contained headless resume scopes its write boundary to the
// worktree it resumes in, not to the process cwd.
// trace:BUG-1971 | ai:claude
#[test]
fn contained_resume_argv_scopes_boundary_to_its_worktree() {
    let (_tmp, _parent, worktree) = pool_fixture();
    let args =
        crate::session::claude_headless_resume_args_with_posture("proceed", "sid", Some(&worktree))
            .unwrap();
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-mode", "dontAsk"]),
        "{args:?}"
    );
    let expected: Vec<String> = WRITE_TOOLS
        .iter()
        .map(|tool| format!("{tool}(/{}/**)", worktree.display()))
        .collect();
    assert_eq!(allow_rules_in_flags(&args), expected);
    // The process cwd (this checkout) is not the boundary.
    let cwd = std::env::current_dir().unwrap();
    let cwd_str = format!("/{}/", cwd.display());
    assert!(
        allow_rules_in_flags(&args)
            .iter()
            .all(|r| !r.contains(&cwd_str)),
        "{args:?}"
    );
    // BUG-050: the resume takes the shared headless posture on top of the
    // contained boundary (one merged `--settings`, sandbox on).
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    crate::session::assert_headless_posture(&args, true, "contained resume");
    // The uncontained resume carries no contained boundary: its one
    // `--settings` is the headless posture alone (no sandbox, no allow rules).
    let plain =
        crate::session::claude_headless_resume_args_with_posture("proceed", "sid", None).unwrap();
    crate::session::assert_headless_posture(&plain, false, "uncontained resume");
    assert!(no_allow_rules(&plain), "{plain:?}");
    assert!(
        plain.contains(&"bypassPermissions".to_string()),
        "{plain:?}"
    );
}

// trace:BUG-2012 | ai:antigravity
#[test]
fn main_checkout_git_hooks_and_config_are_denied_by_contained_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let home = parent.join("home");

    // 1. Main checkout with a real `.git` directory: `.git/hooks/*` and `.git/config` denied.
    let main_checkout = parent.join("aida-main");
    std::fs::create_dir_all(main_checkout.join(".git/hooks")).unwrap();
    std::fs::create_dir_all(main_checkout.join("src")).unwrap();

    let settings = settings_for(&main_checkout);
    let pre_commit = main_checkout.join(".git/hooks/pre-commit");
    let git_config = main_checkout.join(".git/config");
    let inside_src = main_checkout.join("src/lib.rs");

    for tool in WRITE_TOOLS {
        assert!(
            allowed(&settings, tool, &inside_src, &main_checkout, &home),
            "{tool} inside main checkout src/ must remain allowed: {settings}"
        );
        assert!(
            !attempt_write(&settings, tool, &pre_commit, &main_checkout, &home),
            "{tool} on <main-checkout>/.git/hooks/pre-commit must be denied: {settings}"
        );
        assert!(
            !pre_commit.exists(),
            "{tool} must not create {}",
            pre_commit.display()
        );
        assert!(
            !attempt_write(&settings, tool, &git_config, &main_checkout, &home),
            "{tool} on <main-checkout>/.git/config must be denied: {settings}"
        );
        assert!(
            !git_config.exists(),
            "{tool} must not create {}",
            git_config.display()
        );
    }

    // 2. Main checkout with a `.git` gitfile (`--separate-git-dir`): both `.git` file and
    //    its real gitdir target are denied.
    let gitfile_main = parent.join("aida-main-gitfile");
    let sep_git_dir = gitfile_main.join(".real-gitdir");
    std::fs::create_dir_all(sep_git_dir.join("hooks")).unwrap();
    std::fs::create_dir_all(gitfile_main.join("src")).unwrap();
    std::fs::write(
        gitfile_main.join(".git"),
        format!("gitdir: {}\n", sep_git_dir.display()),
    )
    .unwrap();

    let gitfile_settings = settings_for(&gitfile_main);
    let sep_hook = sep_git_dir.join("hooks/pre-commit");
    let sep_config = sep_git_dir.join("config");
    let gitfile_entry = gitfile_main.join(".git");
    let gitfile_src = gitfile_main.join("src/lib.rs");

    for tool in WRITE_TOOLS {
        assert!(
            allowed(&gitfile_settings, tool, &gitfile_src, &gitfile_main, &home),
            "{tool} inside gitfile main checkout src/ must remain allowed: {gitfile_settings}"
        );
        assert!(
            !attempt_write(&gitfile_settings, tool, &sep_hook, &gitfile_main, &home),
            "{tool} on main gitfile target hooks/pre-commit must be denied: {gitfile_settings}"
        );
        assert!(
            !attempt_write(&gitfile_settings, tool, &sep_config, &gitfile_main, &home),
            "{tool} on main gitfile target config must be denied: {gitfile_settings}"
        );
        assert!(
            !allowed(
                &gitfile_settings,
                tool,
                &gitfile_entry,
                &gitfile_main,
                &home
            ),
            "{tool} on main checkout .git file must be denied: {gitfile_settings}"
        );
    }

    // 3. NEGATIVE: a linked worktree's own gitdir stays allowed when in
    //    write_roots; only its `.git` pointer file is denied (TASK-1989).
    let linked_wt = parent.join("aida-wt-linked");
    let wt_gitdir = main_checkout.join(".git/worktrees/aida-wt-linked");
    std::fs::create_dir_all(&linked_wt).unwrap();
    std::fs::create_dir_all(&wt_gitdir).unwrap();
    std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        linked_wt.join(".git"),
        format!("gitdir: {}\n", wt_gitdir.display()),
    )
    .unwrap();

    let wt_roots = vec![linked_wt.clone(), wt_gitdir.clone()];
    // trace:TASK-1952 | trace:TASK-1989 | ai:claude
    assert_eq!(
        contained_main_checkout_git_deny_paths(&wt_roots),
        vec![linked_wt.join(".git")],
        "a linked worktree denies only its own .git pointer, never its gitdir"
    );
    let wt_settings: serde_json::Value = serde_json::from_str(
        &crate::session::contained_settings_json(&[], false, &wt_roots),
    )
    .unwrap();
    let wt_index = wt_gitdir.join("index");
    let wt_head = wt_gitdir.join("HEAD");
    for tool in WRITE_TOOLS {
        assert!(
            attempt_write(&wt_settings, tool, &wt_index, &linked_wt, &home),
            "{tool} on linked worktree's own gitdir index must stay allowed: {wt_settings}"
        );
        let _ = std::fs::remove_file(&wt_index);
        assert!(
            attempt_write(&wt_settings, tool, &wt_head, &linked_wt, &home),
            "{tool} on linked worktree's own gitdir HEAD must stay allowed: {wt_settings}"
        );
        let _ = std::fs::remove_file(&wt_head);
    }

    // 4. NEGATIVE: a non-checkout `.git` stays allowed (both a directory without `.git`
    //    at launch and a nested `sub/.git` inside a checkout).
    let non_checkout = parent.join("plain-scratch");
    std::fs::create_dir_all(&non_checkout).unwrap();
    assert!(
        contained_main_checkout_git_deny_paths(std::slice::from_ref(&non_checkout)).is_empty(),
        "non-checkout directory must not emit a .git deny rule"
    );
    let non_checkout_settings = settings_for(&non_checkout);
    let scratch_git_config = non_checkout.join(".git/config");
    let nested_git_config = main_checkout.join("nested/.git/config");
    for tool in WRITE_TOOLS {
        assert!(
            attempt_write(
                &non_checkout_settings,
                tool,
                &scratch_git_config,
                &non_checkout,
                &home
            ),
            "{tool} on non-checkout .git/config must stay allowed: {non_checkout_settings}"
        );
        let _ = std::fs::remove_dir_all(non_checkout.join(".git"));
        assert!(
            attempt_write(&settings, tool, &nested_git_config, &main_checkout, &home),
            "{tool} on nested non-checkout .git/config must stay allowed: {settings}"
        );
        let _ = std::fs::remove_dir_all(main_checkout.join("nested"));
    }

    // 5. NEGATIVE: an outside-git `os_wrap` root refuses. Hold the env lock
    // with `AIDA_OS_WRAP` unset: a parallel test pinning it to `0` would turn
    // os_wrap off and skip the refusal. trace:BUG-2169 | ai:claude
    let _os_wrap = crate::test_env::EnvVarGuard::unset("AIDA_OS_WRAP");
    std::fs::create_dir_all(non_checkout.join(".aida")).unwrap();
    std::fs::write(
        non_checkout.join(".aida/config.toml"),
        "[contained]\nos_wrap = true\n",
    )
    .unwrap();
    let err =
        crate::session::os_wrapped_program_and_args(&non_checkout, "/bin/echo", vec!["hi".into()])
            .expect_err("outside-git os_wrap root must refuse");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("not inside a git repository"),
        "expected outside-git refusal diagnostic, got: {msg}"
    );
}

// trace:BUG-2012 | ai:antigravity
#[test]
fn main_checkout_git_denied_even_with_seat_clone_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let home = parent.join("home");

    // A main checkout that a contained session has dressed up as a seat clone:
    // `aida-seat-*` name AND the marker file. Both are writable by the session,
    // so neither may exempt the real `.git`.
    let marked = parent.join("aida-seat-forged");
    std::fs::create_dir_all(marked.join(".git/hooks")).unwrap();
    std::fs::create_dir_all(marked.join("src")).unwrap();
    let marker = marked.join(aida_core::seat_clone_detect::MARKER_RELATIVE);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "forged = true\n").unwrap();
    assert!(
        aida_core::seat_clone_detect::seat_clone_root(&marked).is_some(),
        "fixture must look like a seat clone"
    );

    let denied = contained_main_checkout_git_deny_paths(std::slice::from_ref(&marked));
    assert!(
        denied.contains(&marked.join(".git")),
        "main checkout .git must be denied whatever the seat-clone marker says: {denied:?}"
    );
    assert!(
        crate::session::main_git_deny_paths(&marked).contains(&marked.join(".git")),
        "main_git_deny_paths must include the marked checkout's .git"
    );

    let settings = settings_for(&marked);
    let pre_commit = marked.join(".git/hooks/pre-commit");
    let git_config = marked.join(".git/config");
    for tool in WRITE_TOOLS {
        assert!(
            allowed(&settings, tool, &marked.join("src/lib.rs"), &marked, &home),
            "{tool} inside src/ must remain allowed: {settings}"
        );
        assert!(
            !attempt_write(&settings, tool, &pre_commit, &marked, &home),
            "{tool} on marked checkout .git/hooks/pre-commit must be denied: {settings}"
        );
        assert!(
            !attempt_write(&settings, tool, &git_config, &marked, &home),
            "{tool} on marked checkout .git/config must be denied: {settings}"
        );
    }
}

// TASK-2004: `main_git_deny_paths` denies a main checkout's
// `--separate-git-dir` target, both as written (lexical) and canonical, for an
// absolute target through a symlink and for a relative `../` target.
// trace:TASK-1952 | trace:TASK-2004 | ai:antigravity
#[test]
fn main_git_deny_paths_includes_gitfile_target_lexical_and_canonical() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();

    let real_sep = parent.join("real-sep.git");
    std::fs::create_dir_all(real_sep.join("hooks")).unwrap();
    let sep_link = parent.join("sep-link");
    std::os::unix::fs::symlink(&real_sep, &sep_link).unwrap();
    let main = parent.join("main-abs");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(
        main.join(".git"),
        format!("gitdir: {}\n", sep_link.display()),
    )
    .unwrap();

    let paths = crate::session::main_git_deny_paths(&main);
    assert!(paths.contains(&main.join(".git")), "{paths:?}");
    assert!(
        paths.contains(&sep_link),
        "lexical target missing: {paths:?}"
    );
    assert!(
        paths.contains(&real_sep),
        "canonical target missing: {paths:?}"
    );

    let rel_sep = parent.join("rel-sep.git");
    std::fs::create_dir_all(&rel_sep).unwrap();
    let rel_main = parent.join("main-rel");
    std::fs::create_dir_all(&rel_main).unwrap();
    std::fs::write(rel_main.join(".git"), "gitdir: ../rel-sep.git\n").unwrap();
    let rel_paths = crate::session::main_git_deny_paths(&rel_main);
    assert!(
        rel_paths.contains(&rel_sep),
        "normalized relative target missing: {rel_paths:?}"
    );
    assert!(
        rel_paths
            .iter()
            .all(|p| crate::contained_boundary::root_rule_path(p).is_some()),
        "every deny path must be expressible: {rel_paths:?}"
    );
}

// TASK-2005: a gitdir under a directory named `worktrees` is a linked
// worktree's only when its `commondir` resolves back to the common git dir
// that holds that `worktrees/` directory.
// trace:TASK-1952 | trace:TASK-2005 | ai:antigravity
#[test]
fn linked_worktree_gitdir_requires_commondir_pointing_back() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();

    // Real layout: <common>/worktrees/wt with commondir `../..`.
    let common = parent.join("common.git");
    let linked = common.join("worktrees/wt");
    std::fs::create_dir_all(&linked).unwrap();
    std::fs::write(linked.join("commondir"), "../..\n").unwrap();
    assert!(is_linked_worktree_gitdir(&linked));

    // Absolute commondir pointing back is also valid.
    let linked_abs = common.join("worktrees/wt-abs");
    std::fs::create_dir_all(&linked_abs).unwrap();
    std::fs::write(
        linked_abs.join("commondir"),
        format!("{}\n", common.display()),
    )
    .unwrap();
    assert!(is_linked_worktree_gitdir(&linked_abs));

    // Named `worktrees/<name>` but no commondir: a main checkout's separate gitdir.
    let fake = parent.join("x/worktrees/sep");
    std::fs::create_dir_all(fake.join("hooks")).unwrap();
    assert!(!is_linked_worktree_gitdir(&fake));

    // commondir present but pointing somewhere that does not hold this gitdir.
    let elsewhere = parent.join("elsewhere.git");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let wrong = parent.join("y/worktrees/sep");
    std::fs::create_dir_all(&wrong).unwrap();
    std::fs::write(
        wrong.join("commondir"),
        format!("{}\n", elsewhere.display()),
    )
    .unwrap();
    assert!(!is_linked_worktree_gitdir(&wrong));

    // commondir pointing at a missing directory.
    let dangling = parent.join("z/worktrees/sep");
    std::fs::create_dir_all(&dangling).unwrap();
    std::fs::write(dangling.join("commondir"), "../../missing\n").unwrap();
    assert!(!is_linked_worktree_gitdir(&dangling));

    // A main checkout whose separate gitdir is the fake `worktrees/sep` now has
    // that gitdir denied in both deny sources and in the generated settings.
    let main = parent.join("main-fake-wt");
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::write(main.join(".git"), format!("gitdir: {}\n", fake.display())).unwrap();
    let denied = contained_main_checkout_git_deny_paths(std::slice::from_ref(&main));
    assert!(denied.contains(&fake), "{denied:?}");
    assert!(denied.contains(&main.join(".git")), "{denied:?}");
    assert!(crate::session::main_git_deny_paths(&main).contains(&fake));
    let settings = settings_for(&main);
    let home = parent.join("home");
    for tool in WRITE_TOOLS {
        assert!(
            !attempt_write(
                &settings,
                tool,
                &fake.join("hooks/pre-commit"),
                &main,
                &home
            ),
            "{tool} on fake-worktrees separate gitdir must be denied: {settings}"
        );
    }
}

// TASK-2006: `<root>/.git` as a symlink to a gitfile denies the `.git` link,
// the gitfile it resolves to, and the gitfile's `gitdir:` target.
// trace:TASK-1952 | trace:TASK-2006 | ai:antigravity
#[test]
fn main_checkout_git_symlink_to_gitfile_is_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let home = parent.join("home");

    let sep = parent.join("sep.git");
    std::fs::create_dir_all(sep.join("hooks")).unwrap();
    let gitfile = parent.join("stash/gitfile");
    std::fs::create_dir_all(gitfile.parent().unwrap()).unwrap();
    std::fs::write(&gitfile, format!("gitdir: {}\n", sep.display())).unwrap();
    let main = parent.join("main-symlinked-git");
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::os::unix::fs::symlink(&gitfile, main.join(".git")).unwrap();

    let denied = contained_main_checkout_git_deny_paths(std::slice::from_ref(&main));
    assert!(denied.contains(&main.join(".git")), "{denied:?}");
    assert!(denied.contains(&gitfile), "symlink's gitfile: {denied:?}");
    assert!(denied.contains(&sep), "gitdir target: {denied:?}");

    let settings = settings_for(&main);
    for tool in WRITE_TOOLS {
        assert!(
            allowed(&settings, tool, &main.join("src/lib.rs"), &main, &home),
            "{tool} inside src/ must remain allowed: {settings}"
        );
        assert!(
            !attempt_write(&settings, tool, &sep.join("hooks/pre-commit"), &main, &home),
            "{tool} on symlinked gitfile target hooks must be denied: {settings}"
        );
        assert!(
            !attempt_write(&settings, tool, &sep.join("config"), &main, &home),
            "{tool} on symlinked gitfile target config must be denied: {settings}"
        );
    }
}

// TASK-2007: the BUG-2012 main-checkout denies appear in the OS sandbox's
// `denyWrite` as well as `permissions.deny`, with no store grant present.
// trace:TASK-1952 | trace:TASK-2007 | ai:antigravity
#[test]
fn main_checkout_git_denies_reach_sandbox_deny_write() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();

    let dir_main = parent.join("main-dir");
    std::fs::create_dir_all(dir_main.join(".git/hooks")).unwrap();
    let sep = parent.join("sep.git");
    std::fs::create_dir_all(&sep).unwrap();
    let file_main = parent.join("main-file");
    std::fs::create_dir_all(&file_main).unwrap();
    std::fs::write(
        file_main.join(".git"),
        format!("gitdir: {}\n", sep.display()),
    )
    .unwrap();

    for (main, expected) in [
        (&dir_main, vec![dir_main.join(".git")]),
        (&file_main, vec![file_main.join(".git"), sep.clone()]),
    ] {
        let settings = settings_for(main);
        let deny_write: Vec<String> = settings["sandbox"]["filesystem"]["denyWrite"]
            .as_array()
            .unwrap_or_else(|| panic!("no sandbox denyWrite: {settings}"))
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let deny: Vec<String> = settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        for path in expected {
            let entry = format!("/{}", path.display());
            assert!(
                deny_write.contains(&entry),
                "{entry} missing from sandbox denyWrite: {settings}"
            );
            assert!(
                deny.contains(&format!("Edit({entry}/**)")),
                "{entry} missing from permissions.deny: {settings}"
            );
        }
    }

    // A linked worktree's own gitdir is not added to denyWrite.
    let common = dir_main.join(".git");
    let wt_gitdir = common.join("worktrees/wt");
    std::fs::create_dir_all(&wt_gitdir).unwrap();
    std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
    let wt = parent.join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", wt_gitdir.display()),
    )
    .unwrap();
    let wt_settings = settings_for(&wt);
    let wt_entry = format!("/{}", wt_gitdir.display());
    let wt_deny_write = wt_settings["sandbox"]["filesystem"]["denyWrite"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !wt_deny_write
            .iter()
            .any(|v| v.as_str() == Some(wt_entry.as_str())),
        "linked worktree gitdir must stay writable: {wt_settings}"
    );
}

// trace:BUG-2012 | ai:antigravity
#[test]
fn spawn_claude_headless_resume_forwards_contained_then_some_cwd() {
    let (_tmp, parent, worktree) = pool_fixture();
    std::fs::create_dir_all(worktree.join(".aida-store")).unwrap();
    std::fs::create_dir_all(worktree.join(".aida")).unwrap();
    std::fs::write(
        worktree.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();

    let fake = parent.join("fake-claude-resume");
    let args_out = parent.join("resume-args.txt");
    crate::test_exec::write_executable(
        &fake,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nexit 0\n",
            args_out.display()
        ),
    );
    let log_path = parent.join("resume.log");
    let tee_opts = crate::headless_tee::TeeOptions {
        enabled: false,
        label: None,
    };

    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &worktree,
        "advisor",
        &["implementer"],
        &[
            ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", None),
        ],
    );

    // 1. contained = true -> forwards Some(cwd), producing dontAsk + worktree allow rules
    let status = crate::session::spawn_claude_headless_resume(
        "proceed",
        "sid-1",
        &log_path,
        &worktree,
        &tee_opts,
        true,
        "implementer",
    )
    .unwrap();
    assert!(status.success());
    let captured: Vec<String> = std::fs::read_to_string(&args_out)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert!(
        captured
            .windows(2)
            .any(|w| w == ["--permission-mode", "dontAsk"]),
        "contained resume must pass dontAsk: {captured:?}"
    );
    let expected: Vec<String> = WRITE_TOOLS
        .iter()
        .map(|tool| format!("{tool}(/{}/**)", worktree.display()))
        .collect();
    assert_eq!(allow_rules_in_flags(&captured), expected);
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    crate::session::assert_headless_posture(&captured, true, "spawned contained resume");

    // 2. contained = false -> forwards None: bypassPermissions, and the only
    // `--settings` is the headless posture (no contained boundary).
    let status = crate::session::spawn_claude_headless_resume(
        "proceed",
        "sid-2",
        &log_path,
        &worktree,
        &tee_opts,
        false,
        "implementer",
    )
    .unwrap();
    assert!(status.success());
    let captured_plain: Vec<String> = std::fs::read_to_string(&args_out)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    crate::session::assert_headless_posture(&captured_plain, false, "spawned uncontained resume");
    assert!(
        no_allow_rules(&captured_plain),
        "uncontained resume must carry no contained boundary: {captured_plain:?}"
    );
    assert!(
        captured_plain.contains(&"bypassPermissions".to_string()),
        "uncontained resume must pass bypassPermissions: {captured_plain:?}"
    );
}

// TASK-1989: a linked worktree's `.git` pointer file is denied to every write
// tool (a rewritten pointer would aim the operator's next git command at a
// planted gitdir), while its gitdir and source tree stay writable.
// trace:TASK-1952 | trace:TASK-1989 | ai:claude
#[test]
fn linked_worktree_git_pointer_file_is_denied() {
    let (_tmp, parent, worktree) = pool_fixture();
    let home = parent.join("home");
    let pointer = worktree.join(".git");
    let original = std::fs::read_to_string(&pointer).unwrap();
    let settings = settings_for(&worktree);
    let deny_write = settings["sandbox"]["filesystem"]["denyWrite"].to_string();
    assert!(
        deny_write.contains(&pointer.display().to_string()),
        "the pointer must reach the OS sandbox denyWrite: {settings}"
    );
    for tool in WRITE_TOOLS {
        assert!(
            !attempt_write(&settings, tool, &pointer, &worktree, &home),
            "{tool} on the linked worktree's .git pointer must be denied: {settings}"
        );
        assert_eq!(std::fs::read_to_string(&pointer).unwrap(), original);
        assert!(
            allowed(
                &settings,
                tool,
                &worktree.join("src/lib.rs"),
                &worktree,
                &home
            ),
            "{tool} inside src/ must stay allowed: {settings}"
        );
    }
}

// TASK-2036: a `commondir` planted in a main checkout's `.git` DIRECTORY no
// longer exempts it (a linked worktree's `.git` is never a directory).
// trace:TASK-1952 | trace:TASK-2036 | ai:claude
#[test]
fn planted_commondir_in_main_git_directory_does_not_exempt_it() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let home = parent.join("home");
    let main = parent.join("main-planted-commondir");
    std::fs::create_dir_all(main.join(".git/hooks")).unwrap();
    std::fs::write(main.join(".git/commondir"), "..\n").unwrap();
    let denied = contained_main_checkout_git_deny_paths(std::slice::from_ref(&main));
    assert!(denied.contains(&main.join(".git")), "{denied:?}");
    let settings = settings_for(&main);
    for tool in WRITE_TOOLS {
        assert!(
            !attempt_write(
                &settings,
                tool,
                &main.join(".git/hooks/pre-commit"),
                &main,
                &home
            ),
            "{tool} on a commondir-planted main .git must be denied: {settings}"
        );
    }
}

// TASK-2037: a computed main-checkout deny that no rule can express (a rule
// metacharacter in the gitfile's `gitdir:` target) fails the settings closed:
// no write-root allow rule survives beside the missing deny.
// trace:TASK-1952 | trace:TASK-2037 | ai:claude
#[test]
fn unexpressible_computed_main_git_deny_grants_no_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let sep = parent.join("git[1]/repo.git");
    std::fs::create_dir_all(sep.join("hooks")).unwrap();
    let main = parent.join("main-unexpressible");
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::write(main.join(".git"), format!("gitdir: {}\n", sep.display())).unwrap();
    assert!(contained_main_checkout_git_deny_paths(std::slice::from_ref(&main)).contains(&sep));
    let settings = settings_for(&main);
    let allow = settings["permissions"]["allow"].as_array().unwrap();
    assert!(
        !allow.iter().filter_map(|r| r.as_str()).any(is_write_rule),
        "an unexpressible main .git deny must leave no write grant: {settings}"
    );
    assert!(
        !allowed(
            &settings,
            "Write",
            &main.join("src/lib.rs"),
            &main,
            &parent.join("home")
        ),
        "{settings}"
    );
}
