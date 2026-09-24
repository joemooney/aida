// TASK-1475 (CR-8 acceptance 6 follow-up): coverage for `filing_drift_hint`
// — the filing-provenance staleness signal `aida show` / `aida why` render.
// Pure git, no network, no `gh`/`glab` on PATH.
// trace:TASK-1475 | ai:claude
use super::*;

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    git(&root, &["init", "--initial-branch=main", "--quiet"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "user.name", "Test"]);
    std::fs::write(root.join("README.md"), "init\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "chore: init"]);
    (tmp, root)
}

fn commit(root: &std::path::Path, file: &str, body: &str, subject: &str) -> String {
    let path = root.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", subject]);
    git(root, &["rev-parse", "HEAD"])
}

fn provenance(sha: &str) -> aida_core::FilingProvenance {
    aida_core::FilingProvenance {
        code_sha: Some(sha.to_string()),
        ..Default::default()
    }
}

#[test]
fn no_filed_at_yields_nothing() {
    let (_tmp, root) = init_repo();
    let files = vec![("src/x.rs".to_string(), None)];
    let commits = vec![];
    assert_eq!(filing_drift_hint(&root, None, &files, &commits), None);
}

#[test]
fn filed_at_without_code_sha_yields_nothing() {
    let (_tmp, root) = init_repo();
    let p = aida_core::FilingProvenance::default();
    let files = vec![("src/x.rs".to_string(), None)];
    assert_eq!(filing_drift_hint(&root, Some(&p), &files, &[]), None);
}

#[test]
fn unresolvable_sha_yields_nothing_never_errors() {
    // Simulates a shallow clone / a different repo: a well-formed but
    // nonexistent sha. TASK-1475 AC2.
    let (_tmp, root) = init_repo();
    let p = provenance("0123456789abcdef0123456789abcdef01234567");
    let files = vec![("README.md".to_string(), None)];
    assert_eq!(filing_drift_hint(&root, Some(&p), &files, &[]), None);
}

#[test]
fn no_traced_files_yields_nothing() {
    let (_tmp, root) = init_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    let p = provenance(&head);
    assert_eq!(filing_drift_hint(&root, Some(&p), &[], &[]), None);
}

#[test]
fn zero_commits_since_filing_yields_nothing() {
    let (_tmp, root) = init_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    let p = provenance(&head);
    // Traced file exists but nothing has touched it since filing.
    let files = vec![("README.md".to_string(), None)];
    assert_eq!(filing_drift_hint(&root, Some(&p), &files, &[]), None);
}

#[test]
fn commits_touching_traced_files_are_counted() {
    let (_tmp, root) = init_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    let p = provenance(&head);
    // Two commits after filing: one touches the traced file, one doesn't.
    commit(&root, "src/traced.rs", "v1", "feat: touch traced file");
    commit(&root, "unrelated.txt", "v1", "chore: unrelated churn");
    let files = vec![("src/traced.rs".to_string(), Some("f".to_string()))];
    let hint = filing_drift_hint(&root, Some(&p), &files, &[]).expect("drift hint");
    assert_eq!(
        hint, "1 commit since filing touched 1 traced file",
        "hint: {hint}"
    );
}

#[test]
fn plural_wording_for_multiple_commits_and_files() {
    let (_tmp, root) = init_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    let p = provenance(&head);
    commit(&root, "src/a.rs", "v1", "feat: touch a");
    commit(&root, "src/b.rs", "v1", "feat: touch b");
    let files = vec![
        ("src/a.rs".to_string(), None),
        ("src/b.rs".to_string(), None),
    ];
    let hint = filing_drift_hint(&root, Some(&p), &files, &[]).expect("drift hint");
    assert_eq!(
        hint, "2 commits since filing touched 2 traced files",
        "hint: {hint}"
    );
}

#[test]
fn linked_commit_files_are_folded_into_the_traced_set() {
    let (_tmp, root) = init_repo();
    let head = git(&root, &["rev-parse", "HEAD"]);
    let p = provenance(&head);
    // The spec's own linked (referencing) commit touched `linked.rs` — no
    // separate trace: comment names it, but it should still count as a
    // traced file.
    let linked_sha = commit(&root, "linked.rs", "v1", "feat: linked work (TASK-1)");
    commit(&root, "linked.rs", "v2", "feat: churn on the linked file");
    let commits = vec![(
        linked_sha.clone(),
        linked_sha[..7].to_string(),
        "s".to_string(),
    )];
    let hint = filing_drift_hint(&root, Some(&p), &[], &commits).expect("drift hint");
    assert_eq!(
        hint, "2 commits since filing touched 1 traced file",
        "hint: {hint}"
    );
}
