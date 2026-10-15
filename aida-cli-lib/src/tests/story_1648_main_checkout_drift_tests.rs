//! Primary-checkout drift: porcelain parsing, the session-start summary
//! line, the status-line marker, and the read-only doctor scan.
// trace:STORY-1648 | ai:claude
use super::*;

const CLEAN_MAIN: &str = "# branch.oid 0123456789abcdef0123456789abcdef01234567\n\
# branch.head main\n\
# branch.upstream origin/main\n\
# branch.ab +0 -0\n";

fn with_lines(header: &str, lines: &[&str]) -> String {
    let mut out = header.to_string();
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

fn ts(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap()
}

#[test]
fn clean_checkout_is_silent_everywhere() {
    let drift = parse_porcelain_v2(CLEAN_MAIN);
    assert!(!drift.is_drifted());
    assert_eq!(summary_line(&drift), None);
    assert_eq!(statusline_marker(Some(&drift)), None);
    assert!(doctor_findings(&drift, &[], |_| None).is_empty());
}

#[test]
fn tracked_dirty_reports_count_and_marker() {
    let out = with_lines(
        CLEAN_MAIN,
        &["1 .M N... 100644 100644 100644 aaaa bbbb src/lib.rs"],
    );
    let drift = parse_porcelain_v2(&out);
    assert_eq!(drift.tracked, vec!["src/lib.rs".to_string()]);
    assert_eq!(
        summary_line(&drift).as_deref(),
        Some("main checkout: 1 tracked change, 0 untracked · on main · up to date with origin")
    );
    assert_eq!(statusline_marker(Some(&drift)), Some(STATUSLINE_MARKER));
}

#[test]
fn untracked_only_reports_untracked_count() {
    let out = with_lines(CLEAN_MAIN, &["? fix_1.py", "? logs/"]);
    let drift = parse_porcelain_v2(&out);
    assert!(drift.tracked.is_empty());
    assert_eq!(
        drift.untracked,
        vec!["fix_1.py".to_string(), "logs/".to_string()]
    );
    assert_eq!(
        summary_line(&drift).as_deref(),
        Some("main checkout: 0 tracked changes, 2 untracked · on main · up to date with origin")
    );
}

#[test]
fn off_main_branch_is_drift_even_when_clean() {
    let out = "# branch.oid 0123\n# branch.head feature-x\n";
    let drift = parse_porcelain_v2(out);
    assert_eq!(drift.branch.as_deref(), Some("feature-x"));
    assert_eq!(
        summary_line(&drift).as_deref(),
        Some("main checkout: 0 tracked changes, 0 untracked · on feature-x · no upstream")
    );
    let findings = doctor_findings(&drift, &[], |_| None);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "branch");
    assert_eq!(findings[0].category, CATEGORY);
    assert!(!findings[0].safe_heal);
}

#[test]
fn behind_origin_is_drift() {
    let out = CLEAN_MAIN.replace("+0 -0", "+0 -3");
    let drift = parse_porcelain_v2(&out);
    assert_eq!((drift.ahead, drift.behind), (0, 3));
    assert_eq!(
        summary_line(&drift).as_deref(),
        Some("main checkout: 0 tracked changes, 0 untracked · on main · 3 behind origin")
    );
    let findings = doctor_findings(&drift, &[], |_| None);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, "sync");
}

#[test]
fn ahead_and_behind_both_render() {
    let out = CLEAN_MAIN.replace("+0 -0", "+1 -2");
    let drift = parse_porcelain_v2(&out);
    assert_eq!(
        summary_line(&drift).as_deref(),
        Some("main checkout: 0 tracked changes, 0 untracked · on main · 1 ahead, 2 behind origin")
    );
}

#[test]
fn main_without_upstream_is_not_drift() {
    let drift = parse_porcelain_v2("# branch.oid 0123\n# branch.head main\n");
    assert!(!drift.is_drifted());
    assert_eq!(statusline_marker(Some(&drift)), None);
}

#[test]
fn detached_head_reads_as_detached() {
    let drift = parse_porcelain_v2("# branch.oid 0123\n# branch.head (detached)\n");
    assert_eq!(drift.branch, None);
    assert!(summary_line(&drift)
        .unwrap()
        .contains("· on detached HEAD · no upstream"));
}

#[test]
fn renames_conflicts_and_quoted_paths_parse() {
    let out = with_lines(
        CLEAN_MAIN,
        &[
            "2 R. N... 100644 100644 100644 aaaa bbbb R100 new name.rs\told.rs",
            "u UU N... 100644 100644 100644 100644 h1 h2 h3 conflict.rs",
            "? \"sp ace.txt\"",
        ],
    );
    let drift = parse_porcelain_v2(&out);
    assert_eq!(
        drift.tracked,
        vec!["new name.rs".to_string(), "conflict.rs".to_string()]
    );
    assert_eq!(drift.untracked, vec!["sp ace.txt".to_string()]);
    assert_eq!(
        summary_line(&drift).as_deref(),
        Some("main checkout: 2 tracked changes, 1 untracked · on main · up to date with origin")
    );
}

#[test]
fn missing_probe_renders_no_marker() {
    assert_eq!(statusline_marker(None), None);
}

#[test]
fn attribution_picks_latest_session_started_before_mtime() {
    let windows = vec![
        ("session a (STORY-1)".to_string(), ts(100)),
        ("session b (STORY-2)".to_string(), ts(200)),
        ("session c (STORY-3)".to_string(), ts(300)),
    ];
    assert_eq!(
        attribute_by_mtime(ts(250), &windows),
        Some("session b (STORY-2)")
    );
    assert_eq!(
        attribute_by_mtime(ts(300), &windows),
        Some("session c (STORY-3)")
    );
    assert_eq!(attribute_by_mtime(ts(50), &windows), None);
    assert_eq!(attribute_by_mtime(ts(50), &[]), None);
}

#[test]
fn doctor_lists_untracked_with_best_effort_attribution() {
    let out = with_lines(
        CLEAN_MAIN,
        &[
            "1 M. N... 100644 100644 100644 aaaa bbbb Cargo.toml",
            "? scratch.py",
            "? old.log",
        ],
    );
    let drift = parse_porcelain_v2(&out);
    let windows = vec![("session abc123 (STORY-9)".to_string(), ts(1_000))];
    let findings = doctor_findings(&drift, &windows, |p| match p {
        "scratch.py" => Some(ts(2_000)),
        "old.log" => Some(ts(10)),
        _ => None,
    });
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["tracked", "untracked"]);
    assert!(findings[0].summary.contains("Cargo.toml"));
    let untracked = &findings[1].summary;
    assert!(
        untracked.contains("scratch.py (likely session abc123 (STORY-9))"),
        "{untracked}"
    );
    assert!(untracked.contains("old.log"), "{untracked}");
    assert!(!untracked.contains("old.log (likely"), "{untracked}");
    assert!(findings.iter().all(|f| !f.safe_heal));
}

#[test]
fn doctor_caps_long_path_lists() {
    let lines: Vec<String> = (0..25).map(|i| format!("? f{i}.txt")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let drift = parse_porcelain_v2(&with_lines(CLEAN_MAIN, &refs));
    let findings = doctor_findings(&drift, &[], |_| None);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].summary.contains("25 untracked path(s)"));
    assert!(
        findings[0].summary.ends_with("+5 more"),
        "{}",
        findings[0].summary
    );
}

#[test]
fn doctor_category_aliases_normalize() {
    for alias in [
        "main-checkout-drift",
        "main-checkout",
        "primary-checkout",
        "checkout-drift",
        "MAIN_CHECKOUT_DRIFT",
    ] {
        assert_eq!(crate::normalize_doctor_category(alias).unwrap(), CATEGORY);
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every file under `root` (skipping `.git`) with its bytes.
fn snapshot(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = entry.path();
            if p.file_name().and_then(|n| n.to_str()) == Some(".git") {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

#[test]
fn real_checkout_scan_reports_and_never_modifies_the_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("primary");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("tracked.txt"), "one\n").unwrap();
    git(&root, &["add", "tracked.txt"]);
    git(&root, &["commit", "-q", "-m", "init"]);

    // Clean checkout on main: nothing to report.
    let clean = probe(&root).expect("probe a real repo");
    assert!(!clean.is_drifted(), "{clean:?}");
    assert!(scan(&root).is_empty());

    // Tracked edit plus untracked scratch files.
    std::fs::write(root.join("tracked.txt"), "two\n").unwrap();
    std::fs::write(root.join("scratch.py"), "print(1)\n").unwrap();
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes/run.txt"), "x\n").unwrap();

    let before = snapshot(&root);
    let drift = probe(&root).expect("probe a real repo");
    assert_eq!(drift.tracked, vec!["tracked.txt".to_string()]);
    assert_eq!(
        drift.untracked,
        vec!["notes/".to_string(), "scratch.py".to_string()]
    );
    assert_eq!(statusline_marker(Some(&drift)), Some(STATUSLINE_MARKER));
    let findings = scan(&root);
    let ids: Vec<&str> = findings.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, vec!["tracked", "untracked"]);
    assert_eq!(
        snapshot(&root),
        before,
        "the scan must never touch the tree"
    );
}

#[test]
fn probe_outside_a_repository_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    let not_repo = tmp.path().join("plain");
    std::fs::create_dir_all(&not_repo).unwrap();
    // A temp dir may sit under some ancestor repo; only assert when git
    // itself agrees there is no repository here.
    let inside = std::process::Command::new("git")
        .arg("-C")
        .arg(&not_repo)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !inside {
        assert_eq!(probe(&not_repo), None);
        assert!(statusline_marker(probe(&not_repo).as_ref()).is_none());
    }
}
