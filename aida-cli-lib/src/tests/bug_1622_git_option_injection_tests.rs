//! A user-supplied ref, range or branch never reaches git as an option.
//! Each flag refuses a dash-led value up front, and each git call puts
//! `--end-of-options` (or `--`) before the value, so even a value that got
//! past the check is read as a revision and nothing is written. Every test
//! runs against a scratch repo; nothing touches `~/.aida` or a live store.
// trace:BUG-1622 | ai:claude

use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=Test"])
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A scratch repo on `main` with two commits, plus a `feat` branch at the
/// first one.
fn scratch_repo() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "a").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "one"]);
    git(root, &["branch", "feat"]);
    std::fs::write(root.join("b.txt"), "b").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "two (TASK-1)"]);
    tmp
}

/// `--output=<sink>/pwned`: what git writes to if it reads the value as an
/// option.
fn output_option(sink: &std::path::Path) -> String {
    format!("--output={}", sink.join("pwned").display())
}

fn assert_sink_empty(sink: &std::path::Path, label: &str) {
    let left: Vec<_> = std::fs::read_dir(sink)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{label}: git wrote {left:?}");
}

#[test]
fn trace_and_doc_range_helpers_read_an_option_like_range_as_a_revision() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let bad = output_option(sink.path());
    assert!(read_commits_in_range(root, &bad).is_err());
    assert!(read_diff_for_range(root, &bad).is_err());
    assert!(specs_referenced_in_range(root, &bad, &[]).is_empty());
    let floor = compute_trailer_floor(root, &bad, &["a.txt".to_string()], &mut |_| {
        SpecResolution::Live
    });
    assert_eq!(floor.get("a.txt"), Some(&false));
    assert_sink_empty(sink.path(), "trace range helpers");
    // Ordinary ranges still work.
    assert_eq!(read_commits_in_range(root, "feat..main").unwrap().len(), 1);
    assert!(read_diff_for_range(root, "feat..main")
        .unwrap()
        .contains("b.txt"));
    assert_eq!(
        specs_referenced_in_range(root, "feat..main", &[]),
        vec!["TASK-1".to_string()]
    );
}

#[test]
fn trace_and_doc_range_flags_refuse_option_like_values() {
    let sink = tempfile::TempDir::new().unwrap();
    let bad = output_option(sink.path());
    for err in [
        handle_trace_gate(Some(&bad), true).unwrap_err(),
        handle_trace_coverage(Some(&bad), true, false).unwrap_err(),
        handle_doc_suggest(Some(&bad), true).unwrap_err(),
    ] {
        let err = err.to_string();
        assert!(
            err.contains("--range") && err.contains("starts with `-`"),
            "{err}"
        );
    }
    assert_sink_empty(sink.path(), "--range flags");
}

#[test]
fn harvest_base_diff_reads_an_option_like_base_as_a_revision() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let bad = output_option(sink.path());
    assert!(crate::harvest::git_diff_base_to_head(root, &bad, &[], &[]).is_err());
    assert_sink_empty(sink.path(), "harvest --base");
    assert_eq!(
        crate::harvest::changed_files_base_to_head(root, "feat").unwrap(),
        vec!["b.txt".to_string()]
    );
    // Pathspecs now follow the range rather than swallowing it.
    let patch = crate::harvest::git_diff_base_to_head(root, "feat", &["-U0"], &["*.txt"]).unwrap();
    assert!(patch.contains("b.txt"), "{patch}");
}

#[test]
fn rebase_branch_refuses_an_option_like_value_before_fetching() {
    // `git fetch --upload-pack=<cmd> .` would run <cmd>.
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let marker = sink.path().join("pwned");
    let bad = format!("--upload-pack=touch {};false/.", marker.display());
    let err = aida_core::rebase::detect(root, Some(&bad), true)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--branch") && err.contains("starts with `-`"),
        "{err}"
    );
    assert!(!marker.exists(), "git fetch ran the upload-pack command");
    // A normal ref still resolves.
    let d = aida_core::rebase::detect(root, Some("feat"), false).unwrap();
    assert_eq!((d.ahead, d.behind), (1, 0));
}

#[test]
fn lease_branch_sinks_read_an_option_like_branch_as_a_revision() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let bad = output_option(sink.path());
    assert!(recent_files_for_branch(root, &bad, "1 year ago", 10).is_empty());
    assert_eq!(branch_unshipped_patch_count_default(root, &bad), None);
    assert_sink_empty(sink.path(), "lease branch sinks");
    assert!(!recent_files_for_branch(root, "main", "30 years ago", 10).is_empty());
}

#[test]
fn harness_worktree_register_refuses_an_option_like_branch() {
    let sink = tempfile::TempDir::new().unwrap();
    let bad = output_option(sink.path());
    let err = session_harness_worktree_register(
        "agent-bug-1622",
        &sink.path().display().to_string(),
        None,
        Some(&bad),
        None,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("--branch") && err.contains("starts with `-`"),
        "{err}"
    );
    assert_sink_empty(sink.path(), "harness register --branch");
}
