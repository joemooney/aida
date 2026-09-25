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
    // `aida rebase --branch <remote>/<branch>` split the value at the first
    // `/` and ran `git fetch <remote> <branch>`, so this value became
    // `git fetch --upload-pack=touch pwned;false .`, which runs the command
    // in the repo. The marker has no `/` so the split keeps the payload.
    let tmp = scratch_repo();
    let root = tmp.path();
    let bad = "--upload-pack=touch pwned;false/.";
    // The payload is live: run the pre-fix argv directly and see it fire.
    let (remote, branch) = bad.split_once('/').unwrap();
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["fetch", remote, branch])
        .output()
        .unwrap();
    assert!(
        root.join("pwned").exists(),
        "the payload should fire unguarded"
    );
    std::fs::remove_file(root.join("pwned")).unwrap();

    let err = aida_core::rebase::detect(root, Some(bad), true)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--branch") && err.contains("starts with `-`"),
        "{err}"
    );
    assert!(
        !root.join("pwned").exists(),
        "git fetch ran the upload-pack command"
    );
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

fn head_sha(root: &std::path::Path) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn filing_drift_hint_ignores_a_store_code_sha_that_is_not_a_commit_id() {
    // `filed_at.code_sha` comes from the shared store; a hostile writer set
    // it to `--output=<path>` and `aida show` made git write `<path>..HEAD`.
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let files = vec![("a.txt".to_string(), None)];
    let prov = |sha: &str| aida_core::FilingProvenance {
        code_sha: Some(sha.to_string()),
        ..Default::default()
    };
    for bad in [output_option(sink.path()), "HEAD~1".to_string()] {
        assert_eq!(
            filing_drift_hint(root, Some(&prov(&bad)), &files, &[]),
            None
        );
    }
    assert_sink_empty(sink.path(), "filing_drift_hint");
    // A real commit ID still yields a hint: `b.txt` was committed after
    // the first commit, and it is a traced file here.
    let first = {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "feat"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let files = vec![("b.txt".to_string(), None)];
    let hint = filing_drift_hint(root, Some(&prov(&first)), &files, &[]).expect("a drift hint");
    assert!(hint.starts_with("1 commit"), "{hint}");
}

#[test]
fn rework_head_change_refuses_a_verdict_sha_that_is_not_a_commit_id() {
    // `before` is a verdict's stored reviewed sha: `git diff --quiet
    // --output=<file> HEAD` truncated <file>.
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let victim = sink.path().join("victim");
    std::fs::write(&victim, "keep me").unwrap();
    let head = head_sha(root);
    let bad = format!("--output={}", victim.display());
    assert_eq!(classify_rework_head_change(root, &bad, &head), None);
    assert_eq!(classify_rework_head_change(root, &head, &bad), None);
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    assert_eq!(
        classify_rework_head_change(root, &head, &head),
        Some(ReworkHeadChange::Unchanged)
    );
}

#[test]
fn a_verdict_never_stores_a_reviewed_sha_that_is_not_a_commit_id() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let path = root.join("PR-1.json");
    let build = |sha: &str| {
        crate::review_verdict::build_verdict_object(
            root,
            &path,
            Some("approved"),
            Some(sha),
            Some("topic"),
            None,
            &[],
            "reviewer",
        )
        .unwrap()
    };
    let obj = build("--output=injected.x");
    assert!(
        obj.get("reviewed_sha").is_none_or(|v| v.is_null()),
        "{obj:?}"
    );
    // An unresolvable commit ID (another clone's sha) is still kept.
    let obj = build("abcdef1234567");
    assert_eq!(obj["reviewed_sha"], "abcdef1234567");
    // A resolvable one is expanded to the full sha.
    let head = head_sha(root);
    let obj = build(&head[..10]);
    assert_eq!(obj["reviewed_sha"], head.as_str());
}

#[test]
fn forge_branch_names_never_reach_git_as_options() {
    let tmp = scratch_repo();
    let root = tmp.path();
    let sink = tempfile::TempDir::new().unwrap();
    let bad = output_option(sink.path());
    // `aida pr ship` reads the PR head's commit message.
    assert_eq!(crate::pr_cmd::branch_head_commit_message(root, &bad), None);
    assert_sink_empty(sink.path(), "branch_head_commit_message");
    assert!(crate::pr_cmd::branch_head_commit_message(root, "feat")
        .unwrap()
        .contains("one"));
    // `fetch_branch` (drain rework, PR sync) fetches a forge-named head.
    git(root, &["remote", "add", "origin", "."]);
    let payload = "--upload-pack=touch pwned;false";
    let err = fetch_branch(root, payload, true).unwrap_err().to_string();
    assert!(err.contains("starts with `-`"), "{err}");
    assert!(
        !root.join("pwned").exists(),
        "git fetch ran the upload-pack command"
    );
    fetch_branch(root, "feat", true).unwrap();
}

#[test]
fn store_trailer_must_be_a_commit_id() {
    let good = "Aida-Store: 0123456789abcdef0123456789abcdef01234567\n";
    assert_eq!(
        crate::store_cmd::parse_paired_store_sha(good).as_deref(),
        Some("0123456789abcdef0123456789abcdef01234567")
    );
    for bad in [
        "Aida-Store: --output=injected.x\n",
        "Aida-Store: main\n",
        "\n",
    ] {
        assert_eq!(crate::store_cmd::parse_paired_store_sha(bad), None, "{bad}");
    }
}

#[test]
fn salvage_keeps_an_option_named_untracked_file_a_path() {
    // An untracked file named `--output=<victim>` in a worktree made the
    // salvage `git diff --no-index` overwrite <victim> on `doctor --heal`.
    let tmp = scratch_repo();
    let root = tmp.path();
    let project = tempfile::TempDir::new().unwrap();
    let victim = root.join("victim.txt");
    std::fs::write(&victim, "keep me").unwrap();
    std::fs::write(root.join("--output=victim.txt"), "x").unwrap();
    let patch = salvage_worktree_patch(project.path(), "BUG-1", None, root)
        .unwrap()
        .expect("a salvage patch");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
    let body = std::fs::read_to_string(patch).unwrap();
    assert!(body.contains("--output=victim.txt"), "{body}");
}

#[test]
fn doctor_since_whitespace_only_is_an_error() {
    let tmp = scratch_repo();
    let now = chrono::Utc::now();
    let err = resolve_completed_since_cutoff_at(tmp.path(), "   ", now, &chrono::Utc)
        .unwrap_err()
        .to_string();
    assert!(err.contains("--since") && err.contains("empty"), "{err}");
}

#[test]
fn git_ops_helpers_keep_a_dash_led_branch_from_reading_as_an_option() {
    let tmp = scratch_repo();
    let root = tmp.path();
    assert!(aida_core::git_ops::checkout_branch(root, "--orphan=x").is_err());
    assert_eq!(aida_core::git_ops::current_branch(root).unwrap(), "main");
    aida_core::git_ops::checkout_branch(root, "feat").unwrap();
    assert_eq!(aida_core::git_ops::current_branch(root).unwrap(), "feat");
    git(root, &["remote", "add", "origin", "."]);
    assert!(aida_core::git_ops::fetch_branch_into_local(
        root,
        "origin",
        "--upload-pack=touch pwned;false"
    )
    .is_err());
    assert!(!root.join("pwned").exists());
    assert_eq!(
        aida_core::git_ops::ahead_behind(root, "main", "feat"),
        Some((1, 0))
    );
}
